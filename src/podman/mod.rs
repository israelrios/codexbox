mod assets;
mod image;
mod run;

pub use assets::embedded_image_fingerprint;
pub use image::{
    create_image_export_dir, dry_run_image_export_dir, ensure_image, image_export_env,
    import_exported_images, ImageExportDir, DEFAULT_IMAGE,
};
pub use run::{render_plan, run_plan, PodmanPlan};

fn status_to_string(code: Option<i32>) -> String {
    code.map(|value| value.to_string())
        .unwrap_or_else(|| "signal".to_string())
}
