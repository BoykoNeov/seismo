//! `--capture-ui`: saving the window as a PPM.

use eframe::egui;

use crate::SeismoApp;

impl SeismoApp {
    /// `--capture-ui`: ask egui for a screenshot of the window once it is due, save it when it
    /// arrives, then quit.
    pub(crate) fn capture_window(&mut self, ctx: &egui::Context) {
        let Some((path, after, asked)) = &mut self.capture_ui else {
            return;
        };
        if !*asked && self.started.elapsed().as_secs_f64() >= *after {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            *asked = true;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match write_ppm(path, w as u32, h as u32, &rgba) {
                Ok(()) => tracing::info!(path = %path.display(), w, h, "captured window"),
                Err(e) => tracing::error!(path = %path.display(), "capture failed: {e}"),
            }
            self.capture_ui = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Write tightly packed RGBA8 rows as a binary PPM (P6), dropping alpha.
pub(crate) fn write_ppm(
    path: &std::path::Path,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> std::io::Result<()> {
    let mut out = format!(
        "P6
{w} {h}
255
"
    )
    .into_bytes();
    out.extend(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]]),
    );
    std::fs::write(path, out)
}
