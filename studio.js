// Copy a command block without its prompt characters
for (const button of document.querySelectorAll(".copy")) {
  button.addEventListener("click", async () => {
    const text = button.previousElementSibling.textContent.replace(/^\$ /gm, "").replace(/^PS> /gm, "");
    try {
      await navigator.clipboard.writeText(text);
      window.plausible?.("Copy command", { props: { page: location.pathname } });
      button.classList.add("copied");
      button.setAttribute("aria-label", "Copied");
      setTimeout(() => {
        button.classList.remove("copied");
        button.setAttribute("aria-label", "Copy command");
      }, 1600);
    } catch {
      button.setAttribute("aria-label", "Copy failed, select the command instead");
    }
  });
}

// Show "scroll sideways" only under figures that are wider than their frame
const markOverflow = () => {
  for (const scroll of document.querySelectorAll(".fig .scroll")) {
    scroll.closest(".fig").classList.toggle("overflows", scroll.scrollWidth > scroll.clientWidth + 1);
  }
};
addEventListener("resize", markOverflow);
markOverflow();

// Animate the figures (studio.css): scrub them with the scroll on wide screens with scroll
// timelines, otherwise play each one once when a third of it is on screen. Nothing is hidden
// until a class is set here, so without this script the figures stay as drawn.
const motion = matchMedia("(prefers-reduced-motion: no-preference)");
const wide = matchMedia("(min-width: 900px)");
const scrubs = CSS.supports("animation-timeline: view()") && CSS.supports("timeline-scope: --a");
const plays = "IntersectionObserver" in window;
const setMotion = () => {
  const scrub = motion.matches && scrubs && wide.matches;
  document.documentElement.classList.toggle("motion-scrub", scrub);
  document.documentElement.classList.toggle("motion-play", motion.matches && !scrub && plays);
};
motion.addEventListener("change", setMotion);
wide.addEventListener("change", setMotion);
setMotion();
if (plays) {
  const reveal = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting) {
        entry.target.classList.add("is-visible");
        reveal.unobserve(entry.target);
      }
    }
  }, { threshold: 0.35 });
  for (const fig of document.querySelectorAll(".fig")) if (fig.querySelector(".a")) reveal.observe(fig);
}
