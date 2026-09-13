use grayline_variables::{VariableValue, Variables, references};

/// A parsed template whose layers retain document order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    pub(crate) layers: Vec<Layer>,
}

impl Template {
    /// Returns the top-level layers in back-to-front order.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Whether any text layer reads a timestamp out of `variables`.
    ///
    /// A composition that shows the time stops being true a minute after it
    /// was made, so a caller that keeps a rendered overlay around asks this to
    /// find out whether it has to render again as the clock moves.
    pub fn uses_timestamps(&self, variables: &Variables) -> bool {
        fn any(layers: &[Layer], variables: &Variables) -> bool {
            layers.iter().any(|layer| match layer {
                Layer::Text(text) => {
                    references(&text.text).any(|name| matches!(variables.get(name), Some(VariableValue::Timestamp(_))))
                }
                Layer::Group(group) => any(&group.layers, variables),
                _ => false,
            })
        }
        any(&self.layers, variables)
    }

    /// Whether any text layer reads what the radio is tuned to.
    ///
    /// Asked for the same reason as [`Template::uses_timestamps`]: a
    /// composition that prints the frequency stops being true the moment the
    /// operator tunes, and only a caller that knows the template reads it has
    /// any reason to compose again when the rig moves.
    ///
    /// Named rather than compared against a value, because the frequency and
    /// the band are ordinary text and a number: there is nothing in what they
    /// hold that says where they came from.
    pub fn uses_radio(&self) -> bool {
        fn any(layers: &[Layer]) -> bool {
            layers.iter().any(|layer| match layer {
                Layer::Text(text) => references(&text.text).any(|name| name.starts_with(RADIO_PREFIX)),
                Layer::Group(group) => any(&group.layers),
                _ => false,
            })
        }
        any(&self.layers)
    }

    /// The names this template reads that `variables` cannot answer, in the
    /// order they appear and without repeats.
    ///
    /// A missing variable is a render error, which is right for a name the
    /// template author invented and wrong for one whose value comes from
    /// somewhere the author cannot see. A caller that can stand in for such a
    /// name asks this and fills the gaps before rendering, rather than
    /// discovering them as a failure with a picture already on the screen.
    pub fn missing(&self, variables: &Variables) -> Vec<&str> {
        fn collect<'a>(layers: &'a [Layer], variables: &Variables, found: &mut Vec<&'a str>) {
            for layer in layers {
                match layer {
                    Layer::Text(text) => {
                        for name in references(&text.text) {
                            if variables.get(name).is_none() && !found.contains(&name) {
                                found.push(name);
                            }
                        }
                    }
                    Layer::Group(group) => collect(&group.layers, variables, found),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        collect(&self.layers, variables, &mut found);
        found
    }
}

/// What every variable the radio fills in is named under.
const RADIO_PREFIX: &str = "radio.";

/// A frame-relative or font-relative length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// Percentage of frame width.
    FrameWidth(f64),
    /// Percentage of frame height.
    FrameHeight(f64),
    /// Multiple of the current font size.
    Em(f64),
}

/// The point of a layer placed at its position.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Anchor {
    /// Top-left corner.
    #[default]
    TopLeft,
    /// Center of the top edge.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Geometric center.
    Center,
    /// Bottom-left corner.
    BottomLeft,
    /// Center of the bottom edge.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

/// Image fitting behavior within a specified rectangle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageFit {
    /// Fit entirely within the rectangle while preserving aspect ratio.
    #[default]
    Contain,
    /// Cover the rectangle while preserving aspect ratio and cropping overflow.
    Cover,
    /// Stretch independently in both axes.
    Stretch,
    /// Preserve aspect ratio when only one dimension is specified.
    Preserve,
}

/// An eight-bit RGBA color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Position {
    pub x: Length,
    pub y: Length,
    pub anchor: Anchor,
    pub rotation: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerSize {
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub fit: ImageFit,
    pub radius: Option<Length>,
}

/// The non-rectangular region an image layer is cut down to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Clip {
    /// The largest circle centered in the layer box.
    Circle,
    /// The ellipse inscribed in the layer box.
    Ellipse,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stroke {
    pub color: Color,
    pub width: Length,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Solid(Color),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GradientKind {
    Linear { angle: f64 },
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GradientStop {
    pub offset: f64,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FontStyle {
    #[default]
    Normal,
    Italic,
}

impl FontStyle {
    pub const fn as_svg(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Italic => "italic",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Font {
    pub family: String,
    pub size: Length,
    pub weight: u16,
    pub style: FontStyle,
    pub leading: f64,
}

/// A template layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    /// A caller-resolved PNG asset.
    Image(ImageLayer),
    /// The caller-provided final received image.
    ReceivedImage(ReceivedImageLayer),
    /// Interpolated text, one line per newline it contains.
    Text(TextLayer),
    /// A rectangle.
    Rectangle(RectangleLayer),
    /// An ellipse.
    Ellipse(EllipseLayer),
    /// A line segment.
    Line(LineLayer),
    /// A translated nested layer sequence.
    Group(GroupLayer),
}

/// A referenced image layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageLayer {
    pub(crate) reference: String,
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) clip: Option<Clip>,
}

/// A final received-image layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedImageLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) clip: Option<Clip>,
}

/// An interpolated text layer.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayer {
    pub(crate) text: String,
    pub(crate) position: Position,
    pub(crate) font: Font,
    pub(crate) fill: Paint,
    pub(crate) stroke: Option<Stroke>,
}

/// A rectangle layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RectangleLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Paint>,
    pub(crate) stroke: Option<Stroke>,
}

/// An ellipse layer.
#[derive(Clone, Debug, PartialEq)]
pub struct EllipseLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Paint>,
    pub(crate) stroke: Option<Stroke>,
}

/// A line-segment layer.
#[derive(Clone, Debug, PartialEq)]
pub struct LineLayer {
    pub(crate) start: Position,
    pub(crate) end: Position,
    pub(crate) stroke: Stroke,
}

/// A translated nested layer sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLayer {
    pub(crate) position: Option<Position>,
    pub(crate) layers: Vec<Layer>,
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    fn variables() -> Variables {
        let mut variables = Variables::new();
        variables.insert("station.callsign", VariableValue::Text("JA1ABC".into()));
        variables.insert(
            "tx.timestamp.utc",
            VariableValue::Timestamp(
                date(2026, 8, 4)
                    .at(9, 5, 0, 0)
                    .in_tz("UTC")
                    .expect("UTC is a known time zone"),
            ),
        );
        variables
    }

    fn text_template(text: &str) -> Template {
        Template::parse(&format!(
            "group {{ text {text:?} {{ position x=(fw)0 y=(fh)0; font family=\"Noto Sans\" size=(fh)9 weight=400; fill color=\"#ffffff\"; }} }}"
        ))
        .expect("the template is well formed")
    }

    /// The clock only has to be watched for a template that shows it, and a
    /// nested layer counts the same as a top-level one.
    #[test]
    fn a_timestamp_in_a_group_is_still_found() {
        let variables = variables();
        assert!(text_template("${tx.timestamp.utc:%H:%M}").uses_timestamps(&variables));
        assert!(!text_template("${station.callsign}").uses_timestamps(&variables));
        assert!(!text_template("plain").uses_timestamps(&variables));
    }

    /// The rig only has to be watched for a template that prints what it is
    /// tuned to, and a nested layer counts the same as a top-level one.
    #[test]
    fn a_radio_variable_in_a_group_is_still_found() {
        assert!(text_template("${radio.frequency:.3}").uses_radio());
        assert!(text_template("${radio.band}").uses_radio());
        assert!(!text_template("${station.callsign}").uses_radio());
        assert!(!text_template("plain").uses_radio());
    }

    /// A caller that can stand in for a name has to be told about it whether
    /// the layer reading it is nested or not.
    #[test]
    fn a_name_the_variables_cannot_answer_is_reported_from_inside_a_group() {
        let variables = variables();

        assert_eq!(
            text_template("${contact.name} de ${station.callsign}").missing(&variables),
            ["contact.name"]
        );
        assert!(text_template("${station.callsign}").missing(&variables).is_empty());
        assert!(text_template("plain").missing(&variables).is_empty());
    }

    #[test]
    fn a_name_read_twice_is_reported_once() {
        assert_eq!(
            text_template("${contact.qth} ${contact.qth} ${contact.name}").missing(&variables()),
            ["contact.qth", "contact.name"]
        );
    }

    /// The format after a colon says how to write a value, not which one to
    /// read, so it must not be taken for part of the name.
    #[test]
    fn a_formatted_reference_is_reported_by_its_name_alone() {
        assert_eq!(
            text_template("${contact.since:%Y}").missing(&variables()),
            ["contact.since"]
        );
    }

    /// An escaped interpolation is literal text and reads nothing.
    #[test]
    fn an_escaped_interpolation_is_not_a_name_at_all() {
        assert!(text_template("$${contact.name}").missing(&variables()).is_empty());
    }
}
