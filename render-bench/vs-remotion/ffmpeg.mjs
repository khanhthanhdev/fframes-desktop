import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { execFileSync } from "node:child_process";
import {
  ensureBrowser,
  openBrowser,
  selectComposition,
  renderMedia,
} from "@remotion/renderer";

export async function ffmpegWorker(config) {
  const resolved = await ensureBrowser({
    browserExecutable: config.ffmpeg_chrome,
    logLevel: "error",
  });
  if (!("path" in resolved))
    throw new Error(`Remotion browser unavailable: ${resolved.type}`);
  const chrome = resolved.path;
  const font = await fs.readFile(
    new URL("./media/DMSans-Regular.ttf", import.meta.url)
  );
  const inputProps = {
    fontCss: `@font-face{font-family:"DM Sans";font-style:normal;font-weight:400;src:url(data:font/ttf;base64,${font.toString("base64")}) format("truetype");}`,
  };
  const chromiumOptions = { disableWebSecurity: false };
  const browser = await openBrowser("chrome", {
    browserExecutable: chrome,
    chromiumOptions,
    logLevel: "error",
  });
  try {
    const composition = await selectComposition({
      serveUrl: config.bundle,
      inputProps,
      id: "MixedGrid",
      puppeteerInstance: browser,
      timeoutInMilliseconds: config.timeout,
      logLevel: "error",
    });
    const hardwareEncoding = process.platform === "darwin";
    const encoder = hardwareEncoding ? "h264_videotoolbox" : "libx264";
    let concurrency;
    const encodingCommands = [];
    const encoding = {
      codec: "h264",
      ...(hardwareEncoding ? {} : { x264Preset: "medium" }),
      videoBitrate: "8M",
      gopSize: 30,
      pixelFormat: "yuv420p",
      muted: true,
      hardwareAcceleration: hardwareEncoding ? "required" : "disable",
      colorSpace: "bt601",
      ffmpegOverride: ({ args }) => {
        const videoCodecIndex = args.indexOf("-c:v");
        const selected = videoCodecIndex < 0 ? null : args[videoCodecIndex + 1];
        if (selected && selected !== "copy" && selected !== encoder)
          throw new Error(`unexpected encoder: ${selected}`);
        const command = [
          ...args.slice(0, -1),
          "-threads",
          "1",
          ...(hardwareEncoding && selected === encoder
            ? ["-allow_sw", "0"]
            : []),
          args.at(-1),
        ];
        encodingCommands.push(command);
        return command;
      },
    };
    const options = {
      ...encoding,
      composition,
      serveUrl: config.bundle,
      inputProps,
      puppeteerInstance: browser,
      concurrency: Math.min(18, os.availableParallelism()),
      onStart: ({ resolvedConcurrency }) => {
        concurrency = resolvedConcurrency;
      },
      timeoutInMilliseconds: config.timeout,
      logLevel: "error",
    };
    const warmup = path.join(config.directory, "warmup.mp4");
    await renderMedia({
      ...options,
      frameRange: [0, config.warmup - 1],
      outputLocation: warmup,
    });
    encodingCommands.length = 0;
    const start = performance.now();
    await renderMedia({
      ...options,
      frameRange: [config.warmup, config.warmup + config.frames - 1],
      outputLocation: path.join(config.directory, "export.mp4"),
      onProgress: ({ renderedFrames, encodedFrames }) => {
        fs.writeFile(
          path.join(config.directory, "progress.log"),
          JSON.stringify({ renderedFrames, encodedFrames })
        ).catch(() => {});
      },
    });
    const export_ms = performance.now() - start;
    await fs.unlink(warmup);
    return {
      export_ms,
      encoder,
      hardware_encoding: hardwareEncoding,
      encoding_commands: encodingCommands,
      renderer: "@remotion/renderer renderMedia",
      concurrency,
      browser: execFileSync(chrome, ["--version"], {
        encoding: "utf8",
      }).trim(),
      chromium_options: chromiumOptions,
    };
  } finally {
    await browser.close({ silent: true });
  }
}
