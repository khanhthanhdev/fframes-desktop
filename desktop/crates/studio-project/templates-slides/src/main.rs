use std::sync::Arc;

fn main() -> std::process::ExitCode {
    let directory = match fframes::MediaDirectory::read_folder("media") {
        Ok(value) => value,
        Err(error) => { eprintln!("Media: {error}"); return std::process::ExitCode::FAILURE; }
    };
    let media = match directory.process_media_source() {
        Ok(value) => value,
        Err(error) => { eprintln!("Media: {error}"); return std::process::ExitCode::FAILURE; }
    };
    // Slide text edited in Studio's Props panel; without the file the code's text.
    let props = match fframes_studio_kit::PropsStore::load("props.json") {
        Ok(value) => Arc::new(value),
        Err(error) => { eprintln!("Props: {error}"); return std::process::ExitCode::FAILURE; }
    };
    let video = match studio_video::StudioVideo::new(props) {
        Ok(value) => value,
        Err(error) => { eprintln!("Styles: {error}"); return std::process::ExitCode::FAILURE; }
    };
    // Slide order, trims and splits made in Studio's timeline.
    fframes::cli::new(&video, fframes::RenderOptions { media: Some(&media), ..Default::default() })
        .timing_file("timeline.json")
        .run()
}
