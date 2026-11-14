pub mod detector;
pub mod features;
pub mod image_ops;
pub mod letter_model;
pub mod reader;
pub mod rules;

pub use detector::{
    CrowdPair, NameFilter, NameRowRead, NameShape, NameplateCandidates, NameplateDetector,
    NameplateDetectorSettings, PlateStack,
};
pub use features::{CapBand, LayerCheck, LayerRule, LineMask, NameplateFeatures};
pub use image_ops::{BgrImage, GrayImage};
pub use letter_model::LetterModel;
pub use reader::{NameplateRead, NameplateReader, PieceRead, ReadPiece};
pub use rules::{
    BarMeasurement, ContactCode, DistanceLineRule, DistanceReading, LookAlikes, O0EvidenceTable,
    O0Glyph, O0Measurement, O0Rule, PlateColorSettings, PlateColors, UnderscoreHyphenRule,
};
