use std::{cell::Cell, time::Duration};

use serde_json::{Value, json};

use crate::{
    error::QsoError,
    record::{Record, normalize_callsign},
};

/// How long one lookup may take before the instance is called unreachable.
///
/// Longer than the rig transport allows, because this is a round trip to a web
/// host that may be consulting a callbook of its own, and short enough that a
/// Wavelog that has stopped answering cannot hold the worker across the moment
/// the operator keys up.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the endpoint sits on an installation that rewrites its URLs.
const DIRECT_PATH: &str = "/api/private_lookup";
/// Where it sits on one that does not.
const SCRIPT_PATH: &str = "/index.php/api/private_lookup";

/// Which answer each well-known key is read out of.
///
/// What the operator's logger knows about their own contacts with the station —
/// `call_worked`, `call_confirmed`, `dxcc_confirmed`, `lotw_member` — is left
/// where it is: those describe contacts, and this is a directory of stations.
/// `bearing` is left too, because it is computed from the asking station's own
/// grid rather than being a property of the one asked about, as are `dxcc_lat`
/// and `dxcc_long`, which give the entity's centroid and would contradict a
/// real QTH printed beside them.
const FIELDS: [(&str, &str); 11] = [
    ("name", "name"),
    ("location", "qth"),
    ("gridsquare", "grid"),
    ("dxcc", "dxcc"),
    ("dxcc_id", "dxcc_id"),
    ("dxcc_cqz", "cq_zone"),
    ("cont", "continent"),
    ("state", "state"),
    ("us_county", "county"),
    ("iota_ref", "iota"),
    ("qsl_manager", "qsl_manager"),
];

/// The operator's own Wavelog, asked what it knows about a callsign.
#[derive(Debug)]
pub struct Wavelog {
    agent: ureq::Agent,
    base: String,
    key: String,
    /// Which of the two endpoint paths this installation answered on.
    ///
    /// Installations differ on whether `index.php` is in the path, and which
    /// one is right is a property of the server rather than of the request, so
    /// it is discovered once and then reused.
    path: Cell<&'static str>,
}

impl Wavelog {
    /// Prepares a client for the installation at `base_url`.
    pub fn new(base_url: &str, key: &str, timeout: Duration) -> Result<Self, QsoError> {
        let base = base_url.trim().trim_end_matches('/').to_owned();
        if host_of(&base).is_none() {
            return Err(QsoError::Address(base_url.to_owned()));
        }
        if key.trim().is_empty() {
            return Err(QsoError::NoKey);
        }

        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();
        Ok(Self {
            agent: config.new_agent(),
            base,
            key: key.trim().to_owned(),
            path: Cell::new(DIRECT_PATH),
        })
    }

    /// Asks what the instance knows about `callsign`.
    ///
    /// Answers nothing for a callsign it has nothing to say about, which is not
    /// the same as failing: Wavelog reports one with a successful response whose
    /// fields are empty.
    pub fn look_up(&self, callsign: &str) -> Result<Option<Record>, QsoError> {
        let callsign = normalize_callsign(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;
        // The band and the mode are left out of the request: they decide only
        // the worked and confirmed flags, and this reads none of them.
        let request = json!({ "key": self.key, "callsign": callsign, "callbook": true });

        let known = self.path.get();
        let body = match self.send(known, &request) {
            Err(QsoError::Refused(404)) if known == DIRECT_PATH => {
                let body = self.send(SCRIPT_PATH, &request)?;
                self.path.set(SCRIPT_PATH);
                body
            }
            other => other?,
        };
        record_from(&body, &callsign)
    }

    fn send(&self, path: &str, request: &Value) -> Result<String, QsoError> {
        let url = format!("{}{path}", self.base);
        // The body is serialized here rather than through `ureq`'s own JSON
        // helpers, which would want a `Deserialize` type for the answer; this
        // crate reads a foreign document field by field instead.
        let body = request.to_string();
        let mut response = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body)
            .map_err(|error| {
                // The host is named and the request is not: the body carries
                // the API key, and an error quoting the exchange would put it
                // wherever the interface prints errors.
                QsoError::Connect {
                    host: host_of(&self.base).unwrap_or_default().to_owned(),
                    detail: error.to_string(),
                }
            })?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(QsoError::Refused(status));
        }
        response.body_mut().read_to_string().map_err(|_| QsoError::Unreadable)
    }
}

/// The host part of a base URL, or nothing when it names none.
fn host_of(base: &str) -> Option<&str> {
    let rest = base.strip_prefix("https://").or_else(|| base.strip_prefix("http://"))?;
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

fn record_from(body: &str, callsign: &str) -> Result<Option<Record>, QsoError> {
    let answer: Value = serde_json::from_str(body).map_err(|_| QsoError::Unreadable)?;
    let mut record = Record::new(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;
    for (from, to) in FIELDS {
        if let Some(value) = text(&answer, from) {
            record.set(to, &value);
        }
    }
    Ok((!record.is_empty()).then_some(record))
}

/// One field of the answer, as text, or nothing when it says nothing.
///
/// An absent field, a null, and an empty string are the same answer: the
/// instance does not know. Dropping the empty one rather than filing it matters
/// because a stored empty value would read back as an answer and would occupy
/// the place a later real one should fill.
fn text(answer: &Value, key: &str) -> Option<String> {
    match answer.get(key)? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
    };

    use rstest::rstest;

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    const FULL_ANSWER: &str = r#"{
        "callsign": "JA1ABC", "dxcc": "JAPAN", "dxcc_id": "339", "dxcc_cqz": 25,
        "cont": "AS", "name": "Taro", "gridsquare": "PM95TQ", "location": "Tokyo",
        "iota_ref": "AS-007", "state": "", "us_county": null, "qsl_manager": "JH1XYZ",
        "bearing": "42", "dxcc_lat": "36", "dxcc_long": "138",
        "call_worked": true, "lotw_member": false, "dxcc_confirmed": true
    }"#;

    /// A stand-in for a Wavelog installation, over plain HTTP.
    ///
    /// Plain HTTP because what is under test is the request, the paths and the
    /// mapping; TLS is the transport's business and a certificate would only
    /// make the test harder to run.
    struct FakeWavelog {
        base: String,
        requested: Arc<Mutex<Vec<(String, String)>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeWavelog {
        /// Serves one canned answer per request, in the order given.
        ///
        /// Each answer is a status and a body; a request past the end of the
        /// list is left unanswered, which is what a test asserting that no
        /// second request was made relies on.
        fn spawn(answers: &[(u16, &str)]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            let base = format!("http://{}", listener.local_addr().expect("an address"));
            let requested = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&requested);
            let answers: Vec<(u16, String)> = answers
                .iter()
                .map(|(status, body)| (*status, (*body).to_owned()))
                .collect();

            let thread = thread::spawn(move || {
                for (status, body) in answers {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
                    let mut start = String::new();
                    if reader.read_line(&mut start).is_err() {
                        return;
                    }
                    let path = start.split(' ').nth(1).unwrap_or_default().to_owned();

                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                            break;
                        }
                        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut sent = vec![0; length];
                    let sent = match std::io::Read::read_exact(&mut reader, &mut sent) {
                        Ok(()) => String::from_utf8_lossy(&sent).into_owned(),
                        Err(_) => String::new(),
                    };
                    recorder.lock().expect("the recorder").push((path, sent));

                    let answer = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(answer.as_bytes());
                    let _ = stream.flush();
                }
            });

            Self {
                base,
                requested,
                thread: Some(thread),
            }
        }

        fn client(&self) -> Wavelog {
            Wavelog::new(&self.base, "secret", TEST_TIMEOUT).expect("a client")
        }

        fn requested(&self) -> Vec<(String, String)> {
            self.requested.lock().expect("the recorder").clone()
        }
    }

    impl Drop for FakeWavelog {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[test]
    fn an_answer_is_mapped_onto_the_well_known_keys() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER)]);
        let record = server
            .client()
            .look_up("ja1abc")
            .expect("an answer")
            .expect("it is known");

        assert_eq!(record.callsign(), "JA1ABC");
        assert_eq!(record.get("name"), Some("Taro"));
        assert_eq!(record.get("qth"), Some("Tokyo"));
        assert_eq!(record.get("grid"), Some("PM95TQ"));
        assert_eq!(record.get("dxcc"), Some("JAPAN"));
        assert_eq!(record.get("dxcc_id"), Some("339"));
        assert_eq!(record.get("continent"), Some("AS"));
        assert_eq!(record.get("iota"), Some("AS-007"));
        assert_eq!(record.get("qsl_manager"), Some("JH1XYZ"));
    }

    /// A number in the answer is a value like any other; Wavelog is not
    /// consistent about which fields it quotes.
    #[test]
    fn a_field_answered_as_a_number_is_read_as_text() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER)]);
        let record = server
            .client()
            .look_up("JA1ABC")
            .expect("an answer")
            .expect("it is known");

        assert_eq!(record.get("cq_zone"), Some("25"));
    }

    /// Everything the answer says about contacts is left where it is.
    #[test]
    fn what_the_logger_knows_about_contacts_is_not_taken_up() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER)]);
        let record = server
            .client()
            .look_up("JA1ABC")
            .expect("an answer")
            .expect("it is known");

        for key in ["call_worked", "lotw_member", "dxcc_confirmed", "bearing", "dxcc_lat"] {
            assert_eq!(record.get(key), None, "{key}");
        }
    }

    #[rstest]
    #[case(r#"{"callsign":"JA1ABC","name":"","gridsquare":null,"location":""}"#)]
    #[case("{}")]
    fn an_answer_holding_nothing_reads_as_a_station_the_instance_does_not_know(#[case] body: &str) {
        let server = FakeWavelog::spawn(&[(200, body)]);

        assert_eq!(server.client().look_up("JA1ABC").expect("an answer"), None);
    }

    #[test]
    fn the_request_carries_the_key_and_the_callsign_and_nothing_of_a_contact() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER)]);
        server.client().look_up("ja1abc").expect("an answer");

        let (path, body) = server.requested().into_iter().next().expect("one request");
        assert_eq!(path, DIRECT_PATH);
        let sent: Value = serde_json::from_str(&body).expect("a JSON request");
        assert_eq!(sent["key"], "secret");
        assert_eq!(sent["callsign"], "JA1ABC");
        assert_eq!(sent["band"], Value::Null);
        assert_eq!(sent["mode"], Value::Null);
    }

    /// Installations differ on whether `index.php` is in the path, so a 404 on
    /// the first is a question about the installation rather than an answer.
    #[test]
    fn an_installation_that_keeps_index_php_in_the_path_is_found_and_remembered() {
        let server = FakeWavelog::spawn(&[(404, ""), (200, FULL_ANSWER), (200, FULL_ANSWER)]);
        let client = server.client();

        assert!(client.look_up("JA1ABC").expect("an answer").is_some());
        assert!(client.look_up("JH1XYZ").expect("an answer").is_some());

        let paths: Vec<String> = server.requested().into_iter().map(|(path, _)| path).collect();
        assert_eq!(paths, [DIRECT_PATH, SCRIPT_PATH, SCRIPT_PATH]);
    }

    #[test]
    fn a_refused_lookup_names_the_status_and_not_the_request() {
        let server = FakeWavelog::spawn(&[(401, r#"{"status":"failed"}"#)]);
        let error = server.client().look_up("JA1ABC").expect_err("a refusal");

        assert!(matches!(error, QsoError::Refused(401)), "{error}");
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn an_answer_that_is_not_json_is_reported_rather_than_read() {
        let server = FakeWavelog::spawn(&[(200, "<html>login</html>")]);
        let error = server.client().look_up("JA1ABC").expect_err("a refusal");

        assert!(matches!(error, QsoError::Unreadable), "{error}");
    }

    #[test]
    fn an_instance_that_is_not_listening_names_the_host_and_not_the_key() {
        // Port zero is never listening, and the client is built by hand because
        // there is no server to take the address from.
        let client = Wavelog::new("http://127.0.0.1:1", "secret", Duration::from_millis(200)).expect("a client");
        let error = client.look_up("JA1ABC").expect_err("a failure");

        assert!(matches!(error, QsoError::Connect { .. }), "{error}");
        assert!(!error.to_string().contains("secret"));
    }

    #[rstest]
    #[case("")]
    #[case("log.example.org")]
    #[case("https://")]
    fn a_base_url_naming_no_host_is_refused(#[case] base: &str) {
        let error = Wavelog::new(base, "secret", TEST_TIMEOUT).expect_err("a refusal");
        assert!(matches!(error, QsoError::Address(_)), "{error}");
    }

    #[test]
    fn a_client_with_no_key_is_refused_rather_than_left_to_fail_later() {
        let error = Wavelog::new("https://log.example.org", "  ", TEST_TIMEOUT).expect_err("a refusal");
        assert!(matches!(error, QsoError::NoKey), "{error}");
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let client = Wavelog::new("https://log.example.org/", "secret", TEST_TIMEOUT).expect("a client");
        assert_eq!(client.base, "https://log.example.org");
    }
}
