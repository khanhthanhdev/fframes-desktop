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
