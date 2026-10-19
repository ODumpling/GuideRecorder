(function () {
  "use strict";
  var root = document.getElementById("wt");
  if (!root) return;
  var slides = Array.prototype.slice.call(root.querySelectorAll(".wt-slide"));
  var links = Array.prototype.slice.call(root.querySelectorAll(".wt-list a[data-slide]"));
  var bar = document.getElementById("wt-bar");
  var status = document.getElementById("wt-status");
  var backs = root.querySelectorAll(".wt-controls [data-back]");
  var nexts = root.querySelectorAll(".wt-controls [data-next]");
  var current = 0;
  if (!slides.length) return;
  document.documentElement.classList.add("wt-js");

  function setDisabled(buttons, disabled) {
    for (var i = 0; i < buttons.length; i++) {
      if (disabled) buttons[i].setAttribute("aria-disabled", "true");
      else buttons[i].removeAttribute("aria-disabled");
    }
  }

  // Shows slide `index`. Focus moves to its heading when `focus` is
  // "content", or when it is "keep" and the focused element was hidden
  // with the old slide; "none" leaves it (on load).
  function show(index, focus) {
    index = Math.max(0, Math.min(slides.length - 1, index));
    var active = document.activeElement;
    slides.forEach(function (slide, i) {
      slide.hidden = i !== index;
    });
    links.forEach(function (link) {
      if (Number(link.getAttribute("data-slide")) === index) link.setAttribute("aria-current", "step");
      else link.removeAttribute("aria-current");
    });
    current = index;
    var last = slides.length - 1;
    bar.style.width = (last > 0 ? (index / last) * 100 : 100) + "%";
    setDisabled(backs, index === 0);
    setDisabled(nexts, index === last);
    status.textContent = slides[index].getAttribute("data-status") || "";
    var lost = !active || active === document.body || !active.offsetParent;
    if (focus === "content" || (focus === "keep" && lost)) {
      var heading = slides[index].querySelector(".wt-heading");
      if (heading) heading.focus();
    }
    try {
      history.replaceState(null, "", "#" + slides[index].id);
    } catch (e) {
      // Not allowed here (e.g. a sandboxed frame); the hash only helps reloads.
    }
  }

  root.addEventListener("click", function (event) {
    var el = event.target.closest("[data-next], [data-back], [data-restart], a[data-slide]");
    if (!el || !root.contains(el)) return;
    event.preventDefault();
    if (el.getAttribute("aria-disabled") === "true") return;
    if (el.hasAttribute("data-slide")) show(Number(el.getAttribute("data-slide")), "content");
    else if (el.hasAttribute("data-restart")) show(0, "content");
    else if (el.hasAttribute("data-back")) show(current - 1, "keep");
    // Back and Next keep focus, so they can be pressed again; a target goes.
    else show(current + 1, "keep");
  });

  document.addEventListener("keydown", function (event) {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    var target = event.target;
    // Enter activates focused controls and hotspots on its own.
    var control = target.closest && target.closest("a, button, input, select, textarea, [role=button], [contenteditable]");
    if (event.key === "ArrowRight" || (event.key === "Enter" && !control)) {
      event.preventDefault();
      if (current < slides.length - 1) show(current + 1, "keep");
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      if (current > 0) show(current - 1, "keep");
    }
  });

  var start = slides.findIndex(function (slide) {
    return "#" + slide.id === location.hash;
  });
  show(start < 0 ? 0 : start, "none");
})();
