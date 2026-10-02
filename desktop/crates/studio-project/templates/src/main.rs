fn main() -> std::process::ExitCode {
    let directory = match fframes::MediaDirectory::read_folder("media") {
        Ok(value) => value,
        Err(error) => { eprintln!("Media: {error}"); return std::process::ExitCode::FAILURE; }
    };
    let media = match directory.process_media_source() {
        Ok(value) => value,
        Err(error) => { eprintln!("Media: {error}"); return std::process::ExitCode::FAILURE; }
    };
    fframes::cli::new(&studio_video::StudioVideo, fframes::RenderOptions { media: Some(&media), ..Default::default() }).run()
}
