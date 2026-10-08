import fs from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

export async function remotionWorker(config) {
  const font = await fs.readFile(path.join(here, "media/DMSans-Regular.ttf"));
  const fontCss = `@font-face{font-family:"DM Sans";font-style:normal;font-weight:400;src:url(data:font/ttf;base64,${font.toString("base64")}) format("truetype");}`;
  let finish;
  const complete = new Promise(resolve => {
    finish = resolve;
  });
  const server = http.createServer(async (req, res) => {
    try {
      if (req.method === "POST" && req.url === "/result") {
        const parts = [];
        for await (const part of req) parts.push(part);
        const result = JSON.parse(Buffer.concat(parts).toString());
        res.end("ok");
        finish(result);
        return;
      }
      if (req.method === "POST" && req.url === "/progress") {
        const parts = [];
        for await (const part of req) parts.push(part);
        await fs.appendFile(
          path.join(config.directory, "progress.log"),
          `${Buffer.concat(parts)}\n`
        );
        res.end("ok");
        return;
      }
      if (
        req.method === "POST" &&
        ["/save/warmup.mp4", "/save/export.mp4"].includes(req.url)
      ) {
        const file = await fs.open(
          path.join(config.directory, path.basename(req.url)),
          "w"
        );
        try {
          for await (const part of req) await file.writeFile(part);
        } finally {
          await file.close();
        }
        res.end("saved");
        return;
      }
      if (req.url === "/") {
        res.setHeader("Content-Type", "text/html");
        res.end(
          '<!doctype html><html><head></head><body style="margin:0"><script src="/bundle.js"></script></body></html>'
        );
        return;
      }
      if (req.url === "/config") {
        res.setHeader("Content-Type", "application/json");
        res.end(
          JSON.stringify({
            frames: config.frames,
            warmup: config.warmup,
            timeout: config.timeout,
          })
        );
        return;
      }
      if (req.url === "/font.css") {
        res.setHeader("Content-Type", "text/css");
        res.end(fontCss);
        return;
      }
      if (req.url === "/DMSans-Regular.ttf") {
        res.setHeader("Content-Type", "font/ttf");
        res.end(font);
        return;
      }
      const name = decodeURIComponent(req.url.split("?")[0]).slice(1);
      if (!name || name.includes("..")) throw new Error("invalid path");
      res.setHeader(
        "Content-Type",
        name.endsWith(".js") ? "text/javascript" : "application/octet-stream"
      );
      res.end(await fs.readFile(path.join(config.bundle, name)));
    } catch (error) {
      res.statusCode = 404;
      res.end(String(error));
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const profile = await fs.mkdtemp(
    path.join(os.tmpdir(), "fframes-mediabunny-")
  );
  const browserLog = await fs.open(
    path.join(config.directory, "browser.log"),
    "w"
  );
  const browser = spawn(
    config.chrome,
    [
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--new-window",
      `http://127.0.0.1:${server.address().port}`,
    ],
    { stdio: ["ignore", "ignore", browserLog.fd] }
  );
  await browserLog.close();
  browser.on("error", error => finish({ error: String(error) }));
  const exited = new Promise(resolve => browser.once("close", resolve));
  const timer = setTimeout(
    () => finish({ error: "browser export timed out" }),
    config.timeout
  );
  try {
    const result = await complete;
    if (result.error) throw new Error(result.error);
    await fs.unlink(path.join(config.directory, "warmup.mp4"));
    return result;
  } finally {
    clearTimeout(timer);
    browser.kill("SIGTERM");
    await exited;
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await fs.rm(profile, { recursive: true, force: true });
  }
}
