//! Portable transmit-image templates for Grayline SSTV.
//!
//! This crate parses KDL templates and renders them as straight-alpha RGBA
//! overlays. Background preparation and SSTV encoding remain separate.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod image;
mod parser;
mod renderer;
mod scene;

pub use error::{AssetError, TemplateError};
pub use image::{RenderSize, Rgba8, RgbaImage, composite};
pub use renderer::{AssetProvider, EmptyAssetProvider, EncodedAsset, FileAssetProvider, RenderContext, Renderer};
pub use scene::{
    Anchor, Color, EllipseLayer, GroupLayer, ImageFit, ImageLayer, Layer, Length, LineLayer, ReceivedImageLayer,
    RectangleLayer, Template, TextLayer,
};
// Interpolation is the family's rather than this crate's, and is re-exported
// so a caller that renders templates needs only the one dependency.
pub use grayline_variables::{VariableValue, Variables, valid_variable_name};
