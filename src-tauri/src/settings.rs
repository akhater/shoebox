use crate::paths::DataDir;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Long edge, in pixels, of the copy Shoebox keeps of every photo.
    pub preview_size: u32,
    /// JPEG quality of the previews (thumbnails use a fixed 80).
    pub preview_quality: u8,
    /// Height of the grid thumbnails, in pixels.
    pub thumb_height: u32,
    /// Photos read in parallel during a scan.
    pub workers: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { preview_size: 1080, preview_quality: 84, thumb_height: 320, workers: 4 }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, data: &DataDir) -> anyhow::Result<()> {
        data.write(&data.settings_path(), serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    pub fn sanitized(mut self) -> Self {
        self.preview_size = self.preview_size.clamp(480, 4096);
        self.preview_quality = self.preview_quality.clamp(50, 95);
        self.thumb_height = self.thumb_height.clamp(160, 640);
        self.workers = self.workers.clamp(1, 16);
        self
    }
}
