use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PixelRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center_x(&self) -> f64 {
        self.x + self.width * 0.5
    }

    pub fn center_y(&self) -> f64 {
        self.y + self.height * 0.5
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn to_int_rect(&self) -> IntRect {
        IntRect {
            x: self.x.round() as i32,
            y: self.y.round() as i32,
            width: self.width.round().max(0.0) as i32,
            height: self.height.round().max(0.0) as i32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct IntRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl IntRect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn to_pixel_rect(&self) -> PixelRect {
        PixelRect::new(
            self.x as f64,
            self.y as f64,
            self.width as f64,
            self.height as f64,
        )
    }

    pub fn clamp_to(&self, bounds_w: i32, bounds_h: i32) -> Self {
        let x0 = self.x.clamp(0, bounds_w.max(0));
        let y0 = self.y.clamp(0, bounds_h.max(0));
        let x1 = self.right().clamp(0, bounds_w.max(0));
        let y1 = self.bottom().clamp(0, bounds_h.max(0));
        Self::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }

    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Bgr {
    pub b: u8,
    pub g: u8,
    pub r: u8,
}

impl Bgr {
    pub const fn new(b: u8, g: u8, r: u8) -> Self {
        Self { b, g, r }
    }

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OverlayColor {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
    Purple,
    Grey,
}

impl OverlayColor {
    pub const ALL: [OverlayColor; 9] = [
        OverlayColor::Red,
        OverlayColor::Orange,
        OverlayColor::Yellow,
        OverlayColor::Green,
        OverlayColor::Cyan,
        OverlayColor::Blue,
        OverlayColor::Magenta,
        OverlayColor::Purple,
        OverlayColor::Grey,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            OverlayColor::Red => "Red",
            OverlayColor::Orange => "Orange",
            OverlayColor::Yellow => "Yellow",
            OverlayColor::Green => "Green",
            OverlayColor::Cyan => "Cyan",
            OverlayColor::Blue => "Blue",
            OverlayColor::Magenta => "Magenta",
            OverlayColor::Purple => "Purple",
            OverlayColor::Grey => "Grey",
        }
    }

    pub const fn rgb(&self) -> (u8, u8, u8) {
        match self {
            OverlayColor::Red => (0xFF, 0x3B, 0x30),
            OverlayColor::Orange => (0xFF, 0x95, 0x00),
            OverlayColor::Yellow => (0xFF, 0xE6, 0x00),
            OverlayColor::Green => (0x2E, 0xE6, 0x5A),
            OverlayColor::Cyan => (0x00, 0xE5, 0xFF),
            OverlayColor::Blue => (0x3D, 0x8B, 0xFF),
            OverlayColor::Magenta => (0xFF, 0x3D, 0xDB),
            OverlayColor::Purple => (0xA6, 0x6B, 0xFF),
            OverlayColor::Grey => (0xB4, 0xB4, 0xB4),
        }
    }

    pub const fn rgba(&self, alpha: u8) -> [u8; 4] {
        let (r, g, b) = self.rgb();
        [r, g, b, alpha]
    }

    /// UI swatch blended at 60% over `#14161C` (matches IRIS Settings swatch)
    pub fn ui_swatch_rgb(&self) -> (u8, u8, u8) {
        let (r, g, b) = self.rgb();
        let blend = |c: u8, bg: u8| (c as f32 * 0.6 + bg as f32 * 0.4).round() as u8;
        (blend(r, 0x14), blend(g, 0x16), blend(b, 0x1C))
    }
}

impl fmt::Display for OverlayColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OverlayColor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "red" => Ok(OverlayColor::Red),
            "orange" => Ok(OverlayColor::Orange),
            "yellow" => Ok(OverlayColor::Yellow),
            "green" => Ok(OverlayColor::Green),
            "cyan" => Ok(OverlayColor::Cyan),
            "blue" => Ok(OverlayColor::Blue),
            "magenta" => Ok(OverlayColor::Magenta),
            "purple" => Ok(OverlayColor::Purple),
            "grey" | "gray" => Ok(OverlayColor::Grey),
            _ => Err(format!(
                "\"{}\" must be one of Red, Orange, Yellow, Green, Cyan, Blue, Magenta, Purple, Grey.",
                s
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlateColor {
    White,
    Blue,
    Green,
    Red,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MatchKind {
    PlayerList = 0,
    OrgList = 1,
    OrgAttribute = 2,
    NoMatch = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RsiState {
    Unknown,
    Verified,
    NotFound,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgMembership {
    pub sid: Option<String>,
    pub is_main: bool,
    pub archetype: Option<String>,
    pub activities: Vec<String>,
    pub name: Option<String>,
}

impl OrgMembership {
    pub fn new(sid: Option<String>, is_main: bool) -> Self {
        Self {
            sid,
            is_main,
            archetype: None,
            activities: Vec::new(),
            name: None,
        }
    }

    pub fn is_redacted(&self) -> bool {
        self.sid.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorMatch {
    pub color: OverlayColor,
    pub kind: MatchKind,
    pub deciding_org_sid: Option<String>,
    pub deciding_attribute: Option<String>,
}

impl ColorMatch {
    pub fn no_match(color: OverlayColor) -> Self {
        Self {
            color,
            kind: MatchKind::NoMatch,
            deciding_org_sid: None,
            deciding_attribute: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PanelOrgs {
    pub looked_up: bool,
    pub orgs: Vec<OrgMembership>,
    pub deciding_sid: Option<String>,
    pub match_kind: Option<MatchKind>,
    pub attribute_match: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerIdentity {
    pub handle: String,
    pub rsi: RsiState,
    pub orgs: Vec<OrgMembership>,
    pub color_match: ColorMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BracketItem {
    pub marker_id: u64,
    pub handle: String,
    pub name_box: PixelRect,
    pub color: OverlayColor,
    pub kind: MatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelEntry {
    pub handle: String,
    pub color: OverlayColor,
    pub kind: MatchKind,
    pub orgs: PanelOrgs,
    pub note: Option<String>,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MiningLabelItem {
    pub text: String,
    pub signal_box: PixelRect,
    pub digit_height: f64,
    pub text_colour: Bgr,
    pub back_colour: Bgr,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OverlayModel {
    pub window_width: i32,
    pub window_height: i32,
    pub brackets: Vec<BracketItem>,
    pub panel_entries: Vec<PanelEntry>,
    pub mining_labels: Vec<MiningLabelItem>,
    pub toast: Option<String>,
    pub note: Option<String>,
    pub boundary_rect: Option<PixelRect>,
    pub dossier_placeholder: Option<PixelRect>,
}

impl OverlayModel {
    pub fn is_empty(&self) -> bool {
        self.brackets.is_empty()
            && self.panel_entries.is_empty()
            && self.mining_labels.is_empty()
            && self.toast.is_none()
            && self.note.is_none()
            && self.boundary_rect.is_none()
            && self.dossier_placeholder.is_none()
    }
}
