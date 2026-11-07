//! Driver for the HP4 (DS-702 / KD1 / LP4) Bluetooth label printer.

pub mod bt;
pub mod media;
pub mod protocol;
pub mod raster;
pub mod render;

use image::RgbImage;

use media::{Kind, Media};
use protocol::{Encoder, ROW_BYTES};

/// Encode one fitted label (label orientation) as a print job.
pub fn build_job(
    enc: &mut Encoder,
    label: &RgbImage,
    media: &Media,
    density: u8,
    first: bool,
    last: bool,
) -> anyhow::Result<Vec<u8>> {
    let rows = raster::to_rows(label)?;
    Ok(match media.kind {
        Kind::Tape => enc.tape_job(&rows, density, first, last),
        Kind::Gap => {
            let (lead, crop) = raster::gap_placement(media, label.width());
            let rows = &rows[(crop * ROW_BYTES).min(rows.len())..];
            enc.gap_job(rows, density, lead, first, last)
        }
    })
}
