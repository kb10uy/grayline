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

/// Where the v1 endpoint sits on an installation that rewrites its URLs.
const V1_DIRECT_PATH: &str = "/api/private_lookup";
/// Where it sits on one that does not.
const V1_SCRIPT_PATH: &str = "/index.php/api/private_lookup";
/// The same two for v2.
const V2_DIRECT_PATH: &str = "/api/v2/lookup";
const V2_SCRIPT_PATH: &str = "/index.php/api/v2/lookup";

/// What Wavelog prefixes a v2 token with, and a v1 key never carries.
const V2_PREFIX: &str = "wl2_";

/// Which of Wavelog's two APIs a key opens.
///
/// Not a setting, because the key already says which it is: Wavelog issues a
/// v2 token under the `wl2_` prefix, refuses it on the v1 endpoints, and
/// refuses a v1 key on the v2 ones. A version the operator had to name as well
/// would only be a second place to get it wrong, and the wrong answer there
/// looks exactly like a bad key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Api {
    /// `POST /api/private_lookup`, with the key in the body. Wavelog 1.8.6 on.
    V1,
    /// `GET /api/v2/lookup`, with the token as a bearer. Wavelog 3.1.0 on, and
    /// the token needs the `lookup:read` scope.
    V2,
}

impl Api {
    fn of(key: &str) -> Self {
        if key.starts_with(V2_PREFIX) { Self::V2 } else { Self::V1 }
    }

    /// Where this API's lookup sits, on an installation that rewrites its URLs
    /// and on one that does not.
    fn paths(self) -> (&'static str, &'static str) {
        match self {
            Self::V1 => (V1_DIRECT_PATH, V1_SCRIPT_PATH),
            Self::V2 => (V2_DIRECT_PATH, V2_SCRIPT_PATH),
        }
    }
}

/// Which answer each well-known key is read out of.
///
/// Both APIs spell these the same, which is why one table serves them.
///
/// What the operator's logger knows about their own contacts with the station —
/// `call_worked`, `call_confirmed`, `dxcc_confirmed`, `lotw_member`, and v2's
/// `workedBefore` — is left where it is: those describe contacts, and this is a
/// directory of stations.
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
    /// Which API the key opens, decided once from the key itself.
    api: Api,
    /// Which of the two endpoint paths this installation answered on.
    ///
    /// Installations differ on whether `index.php` is in the path, and which
    /// one is right is a property of the server rather than of the request, so
    /// it is discovered once and then reused.
    path: Cell<&'static str>,
}

impl Wavelog {
    /// Prepares a client for the installation at `base_url`.
    ///
    /// Which API it will speak follows from `key`: a `wl2_` token is a v2 one
    /// and anything else is a v1 key.
    pub fn new(base_url: &str, key: &str, timeout: Duration) -> Result<Self, QsoError> {
        let base = base_url.trim().trim_end_matches('/').to_owned();
        if host_of(&base).is_none() {
            return Err(QsoError::Address(base_url.to_owned()));
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(QsoError::NoKey);
        }

        let api = Api::of(key);
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();
        Ok(Self {
            agent: config.new_agent(),
            base,
            key: key.to_owned(),
            api,
            path: Cell::new(api.paths().0),
        })
    }

    /// Asks what the instance knows about `callsign`.
    ///
    /// Answers nothing for a callsign it has nothing to say about, which is not
    /// the same as failing: Wavelog reports one with a successful response whose
    /// fields are empty.
    pub fn look_up(&self, callsign: &str) -> Result<Option<Record>, QsoError> {
        let callsign = normalize_callsign(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;

        let (direct, script) = self.api.paths();
        let known = self.path.get();
        let body = match self.send(known, &callsign) {
            Err(QsoError::Refused(404)) if known == direct => {
                let body = self.send(script, &callsign)?;
                self.path.set(script);
                body
            }
            other => other?,
        };
        record_from(self.api, &body, &callsign)
    }

    fn send(&self, path: &str, callsign: &str) -> Result<String, QsoError> {
        let url = format!("{}{path}", self.base);
        // Neither request names a band or a mode: they decide only the worked
        // and confirmed flags, and this reads none of them. Neither asks for
        // callbook data either, which Wavelog answers in a nested object this
        // does not read and which costs the instance a lookup of its own to
        // fill.
        let sent = match self.api {
            Api::V1 => {
                let request = json!({ "key": self.key, "callsign": callsign });
                self.agent
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .send(request.to_string())
            }
            // `basic` rather than `full`, for the reason above: everything
            // `full` adds is a fact about a contact, and asking for it would
            // set the instance running the per-band confirmation queries that
            // produce it.
            Api::V2 => self
                .agent
                .get(&url)
                .query("callsign", callsign)
                .query("detail", "basic")
                .header("Authorization", format!("Bearer {}", self.key))
                .header("Accept", "application/json")
                .call(),
        };

        let mut response = sent.map_err(|error| {
            // The host is named and the request is not: the key rides in the
            // body on v1 and in a header on v2, and an error quoting the
            // exchange would put it wherever the interface prints errors.
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

fn host_of(base: &str) -> Option<&str> {
    let rest = base.strip_prefix("https://").or_else(|| base.strip_prefix("http://"))?;
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

fn record_from(api: Api, body: &str, callsign: &str) -> Result<Option<Record>, QsoError> {
    let answer: Value = serde_json::from_str(body).map_err(|_| QsoError::Unreadable)?;
    // v1 answers with the fields themselves; v2 wraps every answer in a `data`
    // envelope, and one arriving without it is not an answer this can read.
    let fields = match api {
        Api::V1 => &answer,
        Api::V2 => answer.get("data").ok_or(QsoError::Unreadable)?,
    };
    let mut record = Record::new(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;
    for (from, to) in FIELDS {
        if let Some(value) = text(fields, from) {
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
    use rstest::rstest;

    use super::*;
    use crate::test_util::{FakeWavelog, TEST_TIMEOUT};

    const FULL_ANSWER: &str = r#"{
        "callsign": "JA1ABC", "dxcc": "JAPAN", "dxcc_id": "339", "dxcc_cqz": 25,
        "cont": "AS", "name": "Taro", "gridsquare": "PM95TQ", "location": "Tokyo",
        "iota_ref": "AS-007", "state": "", "us_county": null, "qsl_manager": "JH1XYZ",
        "bearing": "42", "dxcc_lat": "36", "dxcc_long": "138",
        "call_worked": true, "lotw_member": false, "dxcc_confirmed": true
    }"#;

    /// A v2 token, which is any key under the prefix Wavelog issues them with.
    const V2_KEY: &str = "wl2_secret";

    /// The same station as `FULL_ANSWER`, answered by `detail=basic` inside the
    /// envelope v2 wraps every answer in.
    const V2_ANSWER: &str = r#"{
        "data": {
            "callsign": "JA1ABC", "dxcc": "JAPAN", "dxcc_id": "339", "dxcc_cqz": 25,
            "dxcc_flag": "🇯🇵", "cont": "AS", "name": "Taro",
            "gridsquare": "PM95TQ", "location": "Tokyo", "iota_ref": "AS-007",
            "state": "", "us_county": "", "qsl_manager": "JH1XYZ",
            "bearing": "42", "dxcc_lat": "36", "dxcc_long": "138",
            "workedBefore": true, "lotw_member": false, "suffix_slash": ""
        },
        "meta": { "timestamp": "2026-09-23T00:00:00+00:00", "resource": "lookup", "detail": "basic" }
    }"#;

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
    fn the_v1_request_carries_the_key_and_the_callsign_and_nothing_of_a_contact() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER)]);
        server.client().look_up("ja1abc").expect("an answer");

        let asked = server.requested().into_iter().next().expect("one request");
        assert_eq!(asked.path, V1_DIRECT_PATH);
        let sent: Value = serde_json::from_str(&asked.body).expect("a JSON request");
        assert_eq!(sent["key"], "secret");
        assert_eq!(sent["callsign"], "JA1ABC");
        assert_eq!(sent["band"], Value::Null);
        assert_eq!(sent["mode"], Value::Null);
    }

    /// Wavelog answers a callbook lookup in a nested object this does not read,
    /// and filling it costs the instance a round trip of its own.
    #[test]
    fn no_request_asks_the_instance_to_consult_a_callbook() {
        let server = FakeWavelog::spawn(&[(200, FULL_ANSWER), (200, V2_ANSWER)]);
        server.client().look_up("JA1ABC").expect("an answer");
        server.client_with_key(V2_KEY).look_up("JA1ABC").expect("an answer");

        for asked in server.requested() {
            assert!(!asked.body.contains("callbook"), "{}", asked.body);
            assert!(!asked.path.contains("callbook"), "{}", asked.path);
        }
    }

    /// Installations differ on whether `index.php` is in the path, so a 404 on
    /// the first is a question about the installation rather than an answer.
    #[rstest]
    #[case("secret", FULL_ANSWER, V1_DIRECT_PATH, V1_SCRIPT_PATH)]
    #[case(V2_KEY, V2_ANSWER, V2_DIRECT_PATH, V2_SCRIPT_PATH)]
    fn an_installation_that_keeps_index_php_in_the_path_is_found_and_remembered(
        #[case] key: &str,
        #[case] answer: &str,
        #[case] direct: &str,
        #[case] script: &str,
    ) {
        let server = FakeWavelog::spawn(&[(404, ""), (200, answer), (200, answer)]);
        let client = server.client_with_key(key);

        assert!(client.look_up("JA1ABC").expect("an answer").is_some());
        assert!(client.look_up("JH1XYZ").expect("an answer").is_some());

        let asked = server.requested();
        let paths: Vec<&str> = asked
            .iter()
            .map(|asked| asked.path.split('?').next().unwrap_or_default())
            .collect();
        assert_eq!(paths, [direct, script, script]);
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

    #[rstest]
    #[case("secret")]
    #[case(V2_KEY)]
    fn an_instance_that_is_not_listening_names_the_host_and_not_the_key(#[case] key: &str) {
        let client = Wavelog::new("http://127.0.0.1:1", key, Duration::from_millis(200)).expect("a client");
        let error = client.look_up("JA1ABC").expect_err("a failure");

        assert!(matches!(error, QsoError::Connect { .. }), "{error}");
        assert!(!error.to_string().contains("secret"));
    }

    /// The key says which API it opens, so nothing has to be configured and
    /// nothing can be configured wrongly.
    #[rstest]
    #[case("secret", Api::V1)]
    #[case("cafef00d", Api::V1)]
    #[case("WL2_secret", Api::V1)]
    #[case("wl2_secret", Api::V2)]
    #[case("  wl2_secret  ", Api::V2)]
    fn a_key_under_the_v2_prefix_is_a_v2_token_and_anything_else_is_a_v1_key(#[case] key: &str, #[case] expected: Api) {
        let client = Wavelog::new("https://log.example.org", key, TEST_TIMEOUT).expect("a client");
        assert_eq!(client.api, expected);
    }

    #[test]
    fn a_v2_answer_is_read_out_of_the_envelope_onto_the_same_keys() {
        let server = FakeWavelog::spawn(&[(200, V2_ANSWER)]);
        let record = server
            .client_with_key(V2_KEY)
            .look_up("ja1abc")
            .expect("an answer")
            .expect("it is known");

        assert_eq!(record.callsign(), "JA1ABC");
        assert_eq!(record.get("name"), Some("Taro"));
        assert_eq!(record.get("qth"), Some("Tokyo"));
        assert_eq!(record.get("grid"), Some("PM95TQ"));
        assert_eq!(record.get("dxcc"), Some("JAPAN"));
        assert_eq!(record.get("dxcc_id"), Some("339"));
        assert_eq!(record.get("cq_zone"), Some("25"));
        assert_eq!(record.get("continent"), Some("AS"));
        assert_eq!(record.get("iota"), Some("AS-007"));
        assert_eq!(record.get("qsl_manager"), Some("JH1XYZ"));
    }

    #[test]
    fn what_a_v2_answer_says_about_contacts_is_not_taken_up_either() {
        let server = FakeWavelog::spawn(&[(200, V2_ANSWER)]);
        let record = server
            .client_with_key(V2_KEY)
            .look_up("JA1ABC")
            .expect("an answer")
            .expect("it is known");

        for key in ["workedBefore", "lotw_member", "bearing", "dxcc_lat", "dxcc_flag"] {
            assert_eq!(record.get(key), None, "{key}");
        }
    }

    #[test]
    fn a_v2_lookup_asks_for_the_callsign_in_the_query_and_carries_the_token_as_a_bearer() {
        let server = FakeWavelog::spawn(&[(200, V2_ANSWER)]);
        server.client_with_key(V2_KEY).look_up("ja1abc").expect("an answer");

        let asked = server.requested().into_iter().next().expect("one request");
        assert!(asked.path.starts_with(V2_DIRECT_PATH), "{}", asked.path);
        assert!(asked.path.contains("callsign=JA1ABC"), "{}", asked.path);
        // `basic` and not `full`: everything `full` adds is a fact about a
        // contact, which this drops on the way in anyway.
        assert!(asked.path.contains("detail=basic"), "{}", asked.path);
        assert!(!asked.path.contains("band="), "{}", asked.path);
        assert!(!asked.path.contains("mode="), "{}", asked.path);
        assert_eq!(asked.header("Authorization"), Some("Bearer wl2_secret"));
        assert_eq!(asked.body, "");
    }

    /// The key never reaches the query string, where a proxy or an access log
    /// would keep it.
    #[test]
    fn a_v2_lookup_keeps_the_token_out_of_the_url() {
        let server = FakeWavelog::spawn(&[(200, V2_ANSWER)]);
        server.client_with_key(V2_KEY).look_up("JA1ABC").expect("an answer");

        let asked = server.requested().into_iter().next().expect("one request");
        assert!(!asked.path.contains("wl2_secret"), "{}", asked.path);
    }

    #[rstest]
    #[case(r#"{"data":{"callsign":"JA1ABC","name":"","gridsquare":null,"location":""},"meta":{}}"#)]
    #[case(r#"{"data":{},"meta":{}}"#)]
    fn a_v2_answer_holding_nothing_reads_as_a_station_the_instance_does_not_know(#[case] body: &str) {
        let server = FakeWavelog::spawn(&[(200, body)]);

        assert_eq!(
            server.client_with_key(V2_KEY).look_up("JA1ABC").expect("an answer"),
            None
        );
    }

    /// v2 reports a refusal in an envelope of its own, which is not an answer
    /// with no fields in it.
    #[test]
    fn a_v2_error_envelope_is_not_read_as_an_empty_answer() {
        let body =
            r#"{"error":{"code":"insufficient_scope","message":"Token is missing the required scope: lookup:read"}}"#;
        let server = FakeWavelog::spawn(&[(403, body)]);
        let error = server.client_with_key(V2_KEY).look_up("JA1ABC").expect_err("a refusal");

        assert!(matches!(error, QsoError::Refused(403)), "{error}");
        assert!(!error.to_string().contains("wl2_secret"));
    }

    /// A 200 that is JSON but carries no envelope is an installation answering
    /// something other than this endpoint, not a station it knows nothing of.
    #[test]
    fn a_v2_answer_with_no_envelope_is_reported_rather_than_read() {
        let server = FakeWavelog::spawn(&[(200, r#"{"name":"Taro"}"#)]);
        let error = server.client_with_key(V2_KEY).look_up("JA1ABC").expect_err("a refusal");

        assert!(matches!(error, QsoError::Unreadable), "{error}");
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
