import { renderMediaOnWeb } from "@remotion/web-renderer";
import { GridVideo, WIDTH, HEIGHT } from "./scene.jsx";

let fontCss = "";

async function exportVideo(config, name, frameRange) {
  const root = await navigator.storage.getDirectory();
  const start = performance.now();
  const handle = await root.getFileHandle(name, { create: true });
  const stream = await handle.createWritable();
  await renderMediaOnWeb({
    composition: {
      id: "MixedGrid",
      component: GridVideo,
      width: WIDTH,
      height: HEIGHT,
      fps: 30,
      durationInFrames: config.warmup + config.frames,
    },
    inputProps: { fontCss },
    frameRange,
    container: "mp4",
    videoCodec: "h264",
    videoBitrate: 8_000_000,
    hardwareAcceleration: "prefer-hardware",
    keyframeIntervalInSeconds: 1,
    muted: true,
    pageResponsiveness: "disabled",
    delayRenderTimeoutInMilliseconds: config.timeout,
    outputWritable: stream,
    logLevel: "warn",
    onProgress: ({ encodedFrames }) => {
      fetch("/progress", {
        method: "POST",
        body: JSON.stringify({ name, encodedFrames }),
      });
    },
    // Remotion omits duration; give the last frame its full interval in the MP4.
    onFrame: frame => {
      const index = Math.round((frame.timestamp * 30) / 1_000_000);
      fetch("/progress", {
        method: "POST",
        body: JSON.stringify({ name, capturedFrame: index }),
      });
      const duration =
        Math.round(((index + 1) * 1_000_000) / 30) - frame.timestamp;
      return new VideoFrame(frame, { duration });
    },
  });
  const export_ms = performance.now() - start;
  const file = await handle.getFile();
  const response = await fetch(`/save/${name}`, { method: "POST", body: file });
  if (!response.ok)
    throw new Error(`saving ${name} failed: ${response.status}`);
  await root.removeEntry(name);
  return { export_ms, bytes: file.size };
}

async function run() {
  const config = await fetch("/config").then(r => r.json());
  fontCss = await fetch("/font.css").then(r => r.text());
  const gl = document.createElement("canvas").getContext("webgl");
  const debug = gl?.getExtension("WEBGL_debug_renderer_info");
  const support = await VideoEncoder.isConfigSupported({
    codec: "avc1.640033",
    width: WIDTH,
    height: HEIGHT,
    bitrate: 8_000_000,
    framerate: 30,
    hardwareAcceleration: "prefer-hardware",
  });
  if (!support.supported)
    throw new Error(
      "WebCodecs H.264 with prefer-hardware is unavailable in this browser"
    );
  const warmup = await exportVideo(config, "warmup.mp4", [
    0,
    config.warmup - 1,
  ]);
  const result = await exportVideo(config, "export.mp4", [
    config.warmup,
    config.warmup + config.frames - 1,
  ]);
  return {
    ...result,
    encoder: "webcodecs-h264",
    audio: false,
    hardware_acceleration_preference: "prefer-hardware",
    codec_configuration: support.config,
    renderer: "@remotion/web-renderer renderMediaOnWeb",
    browser: navigator.userAgent,
    gpu: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : null,
    output: "OPFS MP4; validation copy excluded",
    explicit_frame_duration: true,
    warmup_ms: warmup.export_ms,
    concurrency: 1,
  };
}

run().then(
  result => fetch("/result", { method: "POST", body: JSON.stringify(result) }),
  error =>
    fetch("/result", {
      method: "POST",
      body: JSON.stringify({ error: String(error), stack: error.stack }),
    })
);
