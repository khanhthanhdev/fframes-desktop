// fframes Studio slide deck player. No dependencies and no network: everything it reads
// is in this folder. Titles and notes are untrusted text and only ever set with
// textContent.
(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const deck = JSON.parse($("deck-data").textContent);
  const slides = Array.isArray(deck.slides) ? deck.slides : [];
  const count = slides.length;
  const fps = deck.fps > 0 ? deck.fps : 30;

  const video = $("video");
  const poster = $("poster");
  const start = $("start");
  const counter = $("counter");
  const notes = $("notes");
  const overview = $("overview");
  const grid = $("grid");

  const hasFrameCallback = "requestVideoFrameCallback" in HTMLVideoElement.prototype;

  // Position: `held` is the stop the video holds on (0 = the first frame, k = the stop of
  // slide k); `target` is the stop it plays to, or null while it holds.
  let held = 0;
  let target = null;
  let started = false;
  // Bumped by every hold and play, so a stale frame watcher stops.
  let generation = 0;
  let counterTimer = 0;

  document.title = deck.title || "Slides";
  $("start-title").textContent = deck.title || "";
  $("overview-title").textContent = deck.title || "Slides";

  const clamp = (k) => Math.max(0, Math.min(count, k | 0));
  const stopFrame = (k) => (k === 0 ? 0 : slides[k - 1].stop_frame);
  // The middle of the stop frame: the picture shown is exactly that frame.
  const holdTime = (k) => (stopFrame(k) + 0.5) / fps;
  // The playing video has reached the stop once it presents the stop frame.
  const reachTime = (k) => (stopFrame(k) - 0.25) / fps;
  const slideTitle = (k) => slides[k - 1].title || `Slide ${k}`;

  function showPoster(k) {
    if (k > 0) {
      poster.src = slides[k - 1].poster;
      poster.hidden = false;
    } else {
      poster.hidden = true;
      poster.removeAttribute("src");
    }
  }

  function hidePosterOnceShown() {
    if (poster.hidden) return;
    const mine = generation;
    const hide = () => {
      if (mine === generation && target === null) poster.hidden = true;
    };
    if (hasFrameCallback) video.requestVideoFrameCallback(hide);
    else requestAnimationFrame(hide);
  }

  function seek(time) {
    if (Math.abs(video.currentTime - time) < 1e-6 && !video.seeking) {
      hidePosterOnceShown();
      return;
    }
    try {
      video.currentTime = time;
    } catch (_) {
      // Not seekable before its metadata; `loadedmetadata` seeks again.
    }
  }

  // Holds on stop `k`: pauses and seeks to the middle of its frame. With `withPoster`
  // the stop's poster shows at once and hides when the video shows the frame.
  function hold(k, withPoster) {
    generation += 1;
    target = null;
    held = clamp(k);
    video.pause();
    if (withPoster) showPoster(held);
    seek(holdTime(held));
    changed();
  }

  function play(k) {
    generation += 1;
    const mine = generation;
    target = k;
    poster.hidden = true;
    const playing = video.play();
    if (playing && typeof playing.catch === "function") {
      // Refused (no user gesture yet) or interrupted: hold where it was.
      playing.catch(() => {
        if (mine === generation) hold(held, false);
      });
    }
    watchFrames(mine, k);
    changed();
  }

  function arrive(mine, k) {
    if (mine === generation) hold(k, false);
  }

  function watchFrames(mine, k) {
    const reach = reachTime(k);
    if (hasFrameCallback) {
      const step = (_now, metadata) => {
        if (mine !== generation) return;
        if (metadata.mediaTime >= reach) arrive(mine, k);
        else video.requestVideoFrameCallback(step);
      };
      video.requestVideoFrameCallback(step);
    } else {
      const step = () => {
        if (mine !== generation) return;
        if (video.currentTime >= reach) arrive(mine, k);
        else requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    }
  }

  function begin() {
    if (started) return;
    started = true;
    start.hidden = true;
  }

  function next() {
    begin();
    if (target !== null) {
      // Pressed again while the slide plays: finish it at once.
      hold(target, true);
    } else if (held < count) {
      play(held + 1);
    }
  }

  function previous() {
    begin();
    if (target !== null) hold(held, true);
    else if (held > 0) hold(held - 1, true);
  }

  function go(k) {
    begin();
    hold(k, true);
  }

  // The slide whose notes and number show: the one playing, the one held, or the first.
  function currentSlide() {
    if (count === 0) return 0;
    return Math.max(1, target !== null ? target : held);
  }

  function changed() {
    const k = currentSlide();
    if (count > 0) {
      counter.textContent = `${k} / ${count}`;
      counter.classList.add("visible");
      clearTimeout(counterTimer);
      counterTimer = setTimeout(() => counter.classList.remove("visible"), 1800);
      $("notes-counter").textContent = `Slide ${k} of ${count}`;
      $("notes-title").textContent = slideTitle(k);
      $("notes-text").textContent = slides[k - 1].notes || "";
    }
    for (const card of grid.querySelectorAll(".card")) {
      card.classList.toggle("current", Number(card.dataset.slide) === k);
    }
    if (target === null) {
      const hash = `#${held}`;
      if (location.hash !== hash) {
        try {
          history.replaceState(null, "", hash);
        } catch (_) {
          // Some browsers refuse history changes for files; the hash stays.
        }
      }
    }
  }

  function buildOverview() {
    if (grid.childElementCount > 0) return;
    const ratio = deck.width > 0 ? deck.height / deck.width : 9 / 16;
    slides.forEach((slide, position) => {
      const k = position + 1;
      const item = document.createElement("li");
      const card = document.createElement("button");
      card.type = "button";
      card.className = "card";
      card.dataset.slide = String(k);
      const image = document.createElement("img");
      image.loading = "lazy";
      image.decoding = "async";
      image.alt = "";
      image.width = 320;
      image.height = Math.max(1, Math.round(320 * ratio));
      image.src = slide.thumb;
      const label = document.createElement("span");
      label.className = "card-label";
      const number = document.createElement("span");
      number.className = "card-number";
      number.textContent = String(k);
      const title = document.createElement("span");
      title.className = "card-title";
      title.textContent = slide.title || "";
      label.append(number, title);
      card.append(image, label);
      card.addEventListener("click", () => {
        closePanel(overview);
        go(k);
      });
      item.append(card);
      grid.append(item);
    });
  }

  function buildPrint() {
    const print = $("print");
    if (print.childElementCount > 0) return;
    slides.forEach((slide, position) => {
      const page = document.createElement("article");
      page.className = "print-page";
      const image = document.createElement("img");
      image.alt = "";
      image.decoding = "async";
      image.src = slide.poster;
      const title = document.createElement("h2");
      title.textContent = slideTitle(position + 1);
      page.append(image, title);
      if (slide.notes) {
        const text = document.createElement("p");
        text.textContent = slide.notes;
        page.append(text);
      }
      print.append(page);
    });
  }

  function togglePanel(panel) {
    if (panel.hidden) openPanel(panel);
    else closePanel(panel);
  }

  function openPanel(panel) {
    if (panel === overview) {
      buildOverview();
      changed();
    }
    panel.hidden = false;
    document.body.classList.toggle("with-notes", !notes.hidden);
    if (panel === overview) {
      const current = grid.querySelector(".card.current") || grid.querySelector(".card");
      if (current) {
        current.focus({ preventScroll: true });
        current.scrollIntoView({ block: "center" });
      }
    }
  }

  function closePanel(panel) {
    panel.hidden = true;
    document.body.classList.toggle("with-notes", !notes.hidden);
  }

  function toggleFullscreen() {
    const root = document.documentElement;
    if (document.fullscreenElement || document.webkitFullscreenElement) {
      (document.exitFullscreen || document.webkitExitFullscreen).call(document);
    } else if (root.requestFullscreen) {
      root.requestFullscreen().catch(() => {});
    } else if (root.webkitRequestFullscreen) {
      root.webkitRequestFullscreen();
    }
  }

  function fromHash() {
    const match = /^#(\d+)$/.exec(location.hash);
    return match ? clamp(Number(match[1])) : null;
  }

  document.addEventListener("keydown", (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const key = event.key;
    let handled = true;
    if (key === "Escape") {
      if (!overview.hidden) closePanel(overview);
      else if (!notes.hidden) closePanel(notes);
      else handled = false;
    } else if (key === "o" || key === "O") {
      togglePanel(overview);
    } else if (key === "s" || key === "S") {
      togglePanel(notes);
    } else if (key === "f" || key === "F") {
      toggleFullscreen();
    } else if (!overview.hidden) {
      // The overview's cards take Tab, Enter and Space.
      handled = false;
    } else if (key === "ArrowRight" || key === " " || key === "PageDown") {
      next();
    } else if (key === "ArrowLeft" || key === "PageUp") {
      previous();
    } else if (key === "Home") {
      go(0);
    } else if (key === "End") {
      go(count);
    } else {
      handled = false;
    }
    if (handled) event.preventDefault();
  });

  $("stage").addEventListener("click", next);
  for (const button of document.querySelectorAll("[data-close]")) {
    button.addEventListener("click", () => closePanel($(button.dataset.close)));
  }

  window.addEventListener("hashchange", () => {
    const k = fromHash();
    if (k !== null && (k !== held || target !== null)) go(k);
  });

  // The last slide ends on the video's last frame; some browsers report the end
  // before presenting it.
  video.addEventListener("ended", () => {
    if (target !== null) hold(target, false);
  });
  video.addEventListener("seeked", () => {
    if (target === null) hidePosterOnceShown();
  });
  video.addEventListener("loadedmetadata", () => {
    if (target === null) seek(holdTime(held));
  });
  window.addEventListener("beforeprint", buildPrint);

  // Starts held on frame 0, or on the stop the URL names.
  const initial = fromHash();
  if (initial !== null && initial > 0) {
    started = true;
    hold(initial, true);
  } else {
    start.hidden = false;
    hold(0, false);
  }
  // Printing needs the posters loaded before the print layout.
  setTimeout(buildPrint, 1500);

  // For tests and tools: the deck's state, read only.
  window.fframesDeck = Object.freeze({
    manifest: deck,
    state: () => ({ held, target, started, currentTime: video.currentTime, paused: video.paused }),
  });
})();
