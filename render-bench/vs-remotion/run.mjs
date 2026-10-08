import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { execFileSync, spawn } from "node:child_process";
import { isolated } from "./process.mjs";
import { createHash } from "node:crypto";
import { bundle } from "@remotion/bundler";
import { remotionWorker } from "./web.mjs";
import { ffmpegWorker } from "./ffmpeg.mjs";
import { engines, summarize, markdown } from "./report.mjs";
const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");
const args = process.argv.slice(2);
if (process.platform === "darwin") {
  const awake = spawn(
    "/usr/bin/caffeinate",
    ["-i", "-w", String(process.pid)],
    {
      stdio: "ignore",
    }
  );
  awake.on("error", error =>
    console.error(`Preventing idle sleep failed: ${error}`)
  );
  awake.unref();
}
const option = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i < 0 ? fallback : args[i + 1];
};
const git = (...a) =>
  execFileSync("git", ["-C", root, ...a], { encoding: "utf8" }).trim();

function inspectVideo(file, plan) {
  const probe = JSON.parse(
    execFileSync(
      "ffprobe",
      [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_frames",
        "-show_entries",
        "stream=codec_name,width,height,pix_fmt,color_range,avg_frame_rate,nb_read_frames,duration:format=size",
        "-of",
        "json",
        file,
      ],
      { encoding: "utf8" }
    )
  );
  const video = probe.streams[0];
  const [numerator, denominator] = video.avg_frame_rate.split("/").map(Number);
  const fps = numerator / denominator;
  const frames = Number(video.nb_read_frames);
  const duration = Number(video.duration);
  if (
    video.codec_name !== "h264" ||
    video.width !== plan.width ||
    video.height !== plan.height ||
    !["yuv420p", "yuvj420p"].includes(video.pix_fmt) ||
    fps !== 30 ||
    frames !== plan.frames ||
    !Number.isFinite(duration) ||
    Math.abs(duration - plan.frames / fps) > 0.001
  )
    throw new Error(`incorrect MP4 output: ${JSON.stringify(video)}`);
  return {
    frames,
    fps,
    duration_s: duration,
    width: video.width,
    height: video.height,
    codec: video.codec_name,
    pixel_format: video.pix_fmt,
    color_range: video.color_range,
    bytes: Number(probe.format.size),
  };
}
async function directoryHash(directory) {
  const hash = createHash("sha256");
  for (const name of (
    await fs.readdir(directory, { recursive: true })
  ).sort()) {
    const file = path.join(directory, name);
    if ((await fs.stat(file)).isFile())
      hash.update(name).update(await fs.readFile(file));
  }
  return hash.digest("hex");
}
if (args[0] === "--worker") {
  const config = JSON.parse(await fs.readFile(args[1], "utf8"));
  const worker =
    config.engine === "remotion-ffmpeg" ? ffmpegWorker : remotionWorker;
  console.log(JSON.stringify(await worker(config)));
} else {
  for (let i = 0; i < args.length; i += 2) {
    if (
      !["--chrome", "--ffmpeg-chrome", "--out", "--binary"].includes(args[i]) ||
      !args[i + 1]
    )
      throw new Error(`unknown or incomplete option: ${args[i]}`);
  }
  const plan = {
    nodes: 100000,
    width: 3840,
    height: 2160,
    rounds: 1,
    frames: 300,
    warmup: 3,
    timeout: 600000,
    backends: ["cpu", "skia-gpu-if-available"],
    engines,
    ffmpeg_chrome: option(
      "ffmpeg-chrome",
      process.env.FFMPEG_CHROME_PATH ?? null
    ),
    chrome: option(
      "chrome",
      process.env.CHROME_PATH ??
        (process.platform === "darwin"
          ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
          : "/usr/bin/google-chrome")
    ),
  };
  const binary = path.resolve(
    option("binary", path.join(root, "target/release/vs-remotion-bench"))
  );
  const out = path.resolve(
    option(
      "out",
      path.join(here, "out", new Date().toISOString().replaceAll(":", "-"))
    )
  );
  await fs.mkdir(out, { recursive: true });
  try {
    await fs.access(path.join(out, "results.json"));
    throw new Error(
      "output already contains results; choose a new --out directory"
    );
  } catch (err) {
    if (err.code !== "ENOENT") throw err;
  }
  const report = {
    schema_version: 15,
    comparison:
      "fframes CPU and Skia GPU vs Remotion + FFmpeg and Remotion + MediaBunny: complete H.264 MP4 export",
    encoding: {
      codec: "h264",
      target_bitrate: 8000000,
      cpu_encoder: "libx264 medium",
      macos_gpu_encoder: "h264_videotoolbox",
      remotion_ffmpeg_encoder:
        process.platform === "darwin" ? "h264_videotoolbox" : "libx264 medium",
      remotion_mediabunny_encoder: "WebCodecs H.264, prefer-hardware",
      gop: 30,
      pixel_format: "yuv420p",
      native_codec_threads: 1,
      fps: 30,
      audio: false,
    },
    workload_revision: "100k-4k-circles-300-frames-v1",
    workload:
      "4K (3840×2160), 99,000 overlapping circles and 1,000 changing DM Sans text digits in 20 panels. Every circle moves and changes color; 10,000 radii animate between 16 and 28 SVG units. A 1000×1000 viewBox stretches to the output. No filters. Remotion uses 100,000 keyed components with useCurrentFrame and a memoized parent tree.",
    plan,
    environment: {
      platform: os.platform(),
      arch: os.arch(),
      cpus: os.cpus()[0]?.model,
      logical_cpus: os.availableParallelism(),
      memory_bytes: os.totalmem(),
      node: process.version,
      remotion: JSON.parse(
        await fs.readFile(path.join(here, "node_modules/remotion/package.json"))
      ).version,
      mediabunny: JSON.parse(
        await fs.readFile(
          path.join(here, "node_modules/mediabunny/package.json")
        )
      ).version,
      react: JSON.parse(
        await fs.readFile(path.join(here, "node_modules/react/package.json"))
      ).version,
      git: git("rev-parse", "HEAD"),
      dirty: !!git("status", "--porcelain"),
      command: process.argv,
      font_sha256: createHash("sha256")
        .update(await fs.readFile(path.join(here, "media/DMSans-Regular.ttf")))
        .digest("hex"),
      binary_sha256: createHash("sha256")
        .update(await fs.readFile(binary))
        .digest("hex"),
    },
    records: [],
  };
  const webBundle = await bundle({
    entryPoint: path.join(here, "browser.jsx"),
    outDir: path.join(out, "remotion-bundle"),
    publicDir: path.join(here, "media"),
    enableCaching: false,
    ignoreRegisterRootWarning: true,
    webpackOverride: config => ({
      ...config,
      entry: path.join(here, "browser.jsx"),
    }),
  });
  const serverBundle = await bundle({
    entryPoint: path.join(here, "server.jsx"),
    outDir: path.join(out, "remotion-ffmpeg-bundle"),
    publicDir: path.join(here, "media"),
    enableCaching: false,
  });
  report.environment.mediabunny_bundle_sha256 = await directoryHash(webBundle);
  report.environment.ffmpeg_bundle_sha256 = await directoryHash(serverBundle);
  const persist = async () => {
    report.summary = summarize(report.records, plan);
    await fs.writeFile(
      path.join(out, "results.json"),
      JSON.stringify(report, null, 2)
    );
    await fs.writeFile(path.join(out, "results.md"), markdown(report));
  };
  for (let round = 0; round < plan.rounds; round++) {
    const nodes = plan.nodes;
    const order = engines.map((_, i) => engines[(i + round) % engines.length]);
    for (const engine of order) {
      const directory = path.join(out, `${nodes}-${engine}-${round}`);
      await fs.mkdir(directory, { recursive: true });
      const config = {
        ...plan,
        nodes,
        engine,
        round,
        directory,
        bundle: engine === "remotion-ffmpeg" ? serverBundle : webBundle,
      };
      const configPath = path.join(directory, "config.json");
      await fs.writeFile(configPath, JSON.stringify(config));
      console.error(
        `round ${round + 1}/${plan.rounds}: ${nodes} nodes, ${engine}`
      );
      const loadBefore = os.loadavg();
      const result = engine.startsWith("fframes")
        ? await isolated(
            binary,
            [engine === "fframes" ? "cpu" : "gpu", directory],
            plan.timeout,
            path.join(directory, "process.log")
          )
        : await isolated(
            process.execPath,
            [fileURLToPath(import.meta.url), "--worker", configPath],
            plan.timeout,
            path.join(directory, "process.log")
          );
      const record = {
        nodes,
        engine,
        round,
        status: result.status,
        load_before: loadBefore,
        load_after: os.loadavg(),
        code: result.code,
        signal: result.signal,
      };
      try {
        if (result.status !== "ok")
          throw new Error(result.error || result.status);
        Object.assign(record, JSON.parse(result.stdout));
        // Verify the completed file after the timer has stopped.
        if (record.status === "ok")
          record.video = inspectVideo(path.join(directory, "export.mp4"), plan);
      } catch (err) {
        record.status =
          result.status === "ok" ? "invalid-output" : result.status;
        record.error = String(err);
      }
      report.records.push(record);
      await persist();
      console.error(`${engine}: ${record.status}`);
    }
  }
  console.log(markdown(report));
  if (report.summary.status !== "complete") process.exitCode = 2;
}
