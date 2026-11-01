// Studio's styles are small enough to deliver with the document. This removes
// stylesheet round trips and keeps first paint, no-JS and print fully styled.
// Edit the CSS source files, then run this script (also part of build:landing).
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

const studio = fileURLToPath(new URL("../landing/studio/", import.meta.url));
// The landing page has its own sky stylesheet; How it works keeps the original three.
for (const [page, sources] of [
  ["index.html", ["sky.css"]],
  ["how-it-works.html", ["home.css", "how-it-works.css", "readability.css"]],
]) {
  const css = sources
    .map(source => readFileSync(join(studio, source), "utf8"))
    .join("\n")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split(/\r?\n/)
    .map(line => line.trim())
    .filter(Boolean)
    .join("\n");
  if (/<\/style/i.test(css))
    throw new Error("Unsafe closing tag in Studio CSS");
  const file = join(studio, page);
  let html = readFileSync(file, "utf8");
  html = html
    .replace(/\s*<!-- studio:fonts -->[\s\S]*?<!-- \/studio:fonts -->/g, "")
    .replace(/\s*<!-- studio:scripts -->[\s\S]*?<!-- \/studio:scripts -->/g, "")
    .replace(
      /\s*<script src="(?:studio|scroll-motion|home|how-it-works|sky)\.js(?:\?[^"]*)?"><\/script>/g,
      ""
    )
    .replace(
      /\s*<!-- studio:bootstrap -->[\s\S]*?<!-- \/studio:bootstrap -->/g,
      ""
    )
    .replace(/\s*<!-- studio:styles -->[\s\S]*?<!-- \/studio:styles -->/g, "")
    .replace(
      /\s*<link\s+rel="stylesheet"\s+href="(?:home|features|how-it-works|readability|sky)\.css(?:\?[^"]*)?"\s*\/?>/g,
      ""
    );
  // Establish the saved motion layout before first paint. The normal controller
  // still owns controls and scroll updates once the document has loaded.
  const motionClass =
    page === "index.html" ? "story-motion" : "internals-motion";
  const bootstrap = `\n    <!-- studio:bootstrap -->\n    <!-- prettier-ignore -->\n    <script data-studio-bootstrap>(()=>{const root=document.documentElement;root.classList.add("js-enabled");let choice=null;try{choice=localStorage.getItem("fframes-studio-scroll-motion")}catch{}if(choice==="on"||(choice!=="off"&&!matchMedia("(prefers-reduced-motion: reduce)").matches))root.classList.add("${motionClass}")})()</script>\n    <!-- /studio:bootstrap -->`;
  const fonts = `\n    <!-- studio:fonts -->${["geist-latin-v5.woff2", "geist-mono-latin-v6.woff2"].map(font => `\n    <link\n      rel="preload"\n      href="fonts/${font}"\n      as="font"\n      type="font/woff2"\n      crossorigin\n    />`).join("")}\n    <!-- /studio:fonts -->`;
  const block = `${fonts}${bootstrap}\n    <!-- studio:styles -->\n    <!-- prettier-ignore -->\n    <style data-studio-styles>\n${css}\n</style>\n    <!-- /studio:styles -->`;
  const anchor = '<meta name="color-scheme" content="light" />';
  if (!html.includes(anchor))
    throw new Error(`Missing styles insertion point: ${page}`);
  html = html.replace(anchor, anchor + block);
  // These small controllers run at the end of the document, after their DOM
  // exists. Deliver them together, preserving the motion controller dependency.
  const scripts =
    page === "index.html"
      ? ["sky.js"]
      : ["scroll-motion.js", "how-it-works.js"];
  const js = scripts
    .map(source => readFileSync(join(studio, source), "utf8").trim())
    .join("\n;\n");
  if (/<\/script/i.test(js))
    throw new Error("Unsafe closing tag in Studio scripts");
  if (!html.includes("</body>"))
    throw new Error(`Missing body insertion point: ${page}`);
  html = html.replace(
    /\s*<\/body>/,
    `\n    <!-- studio:scripts -->\n    <!-- prettier-ignore -->\n    <script data-studio-scripts>\n${js}\n</script>\n    <!-- /studio:scripts -->\n  </body>`
  );
  writeFileSync(file, html);
  console.log(
    `${page}: inline styles ${gzipSync(css).length.toLocaleString()} bytes gzipped; scripts ${gzipSync(js).length.toLocaleString()} bytes gzipped; 0 stylesheet or controller requests`
  );
}
