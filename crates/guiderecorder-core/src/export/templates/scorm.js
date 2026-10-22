// SCORM 1.2 runtime of a GuideRecorder SCORM package (core `export/scorm.rs`).
// Runs before the walkthrough's own script. It finds the LMS's `API` object,
// reports the slide reached (as a bookmark), completion and the quiz score,
// and does nothing with the LMS when there is none, so the page also works on
// its own. All guide text is in the markup; this script holds none.
(function (global, factory) {
  var scorm = factory();
  if (typeof module === "object" && module && module.exports) module.exports = scorm;
  else scorm.start(global, global.document);
})(this, function () {
  "use strict";

  // The SCORM 1.2 API: `API` on this window, a parent frame, or the window
  // that opened this one (and its parents). Frames of another origin are
  // skipped.
  function findApi(win) {
    function search(w) {
      for (var i = 0; w && i < 16; i++) {
        try {
          if (w.API && typeof w.API.LMSInitialize === "function") return w.API;
        } catch (e) {
          // Another origin: look further up.
        }
        var parent = null;
        try {
          parent = w.parent;
        } catch (e) {
          return null;
        }
        if (!parent || parent === w) return null;
        w = parent;
      }
      return null;
    }
    var api = search(win);
    if (!api) {
      try {
        if (win.opener) api = search(win.opener);
      } catch (e) {
        api = null;
      }
    }
    return api;
  }

  // The most cmi.suspend_data a SCORM 1.2 LMS must keep.
  var MAX_SUSPEND_DATA = 4096;

  // `ms` as a SCORM 1.2 CMITimespan, HHHH:MM:SS.SS (hours from 2 digits).
  function sessionTime(ms) {
    var cs = Math.max(0, Math.round(ms / 10));
    var h = Math.min(9999, Math.floor(cs / 360000));
    var m = Math.floor(cs / 6000) % 60;
    var s = Math.floor(cs / 100) % 60;
    var c = cs % 100;
    function pad(n) {
      return n < 10 ? "0" + n : String(n);
    }
    return pad(h) + ":" + pad(m) + ":" + pad(s) + "." + pad(c);
  }

  // The learner's progress, reported through `api` (null: no LMS).
  // `completion` is "last-step" (completed when an end of the walkthrough is
  // reached: the last step, or the end slide a decision may lead to)
  // or "quiz-passed" (passed or failed by the quiz score against `mastery`).
  function Tracker(api, completion, mastery) {
    this.api = null;
    this.completion = completion === "quiz-passed" ? "quiz-passed" : "last-step";
    this.mastery = mastery;
    this.status = "";
    this.finished = false;
    if (api) {
      try {
        if (String(api.LMSInitialize("")) === "true") this.api = api;
      } catch (e) {
        // A broken API: as if there were none.
      }
    }
  }

  Tracker.prototype.get = function (name) {
    if (!this.api) return "";
    try {
      return String(this.api.LMSGetValue(name));
    } catch (e) {
      return "";
    }
  };

  Tracker.prototype.set = function (name, value) {
    if (!this.api) return;
    try {
      this.api.LMSSetValue(name, String(value));
    } catch (e) {
      // Ignored: the LMS keeps what it had.
    }
  };

  Tracker.prototype.commit = function () {
    if (!this.api) return;
    try {
      this.api.LMSCommit("");
    } catch (e) {
      // Ignored: LMSFinish commits too.
    }
  };

  // Starts the attempt: an untouched lesson becomes "incomplete". Returns the
  // bookmark of the last visit ("" for none).
  Tracker.prototype.begin = function () {
    this.status = this.get("cmi.core.lesson_status");
    if (this.status === "" || this.status === "not attempted") this.setStatus("incomplete");
    return this.get("cmi.core.lesson_location");
  };

  Tracker.prototype.setStatus = function (status) {
    if (status === this.status) return;
    this.status = status;
    this.set("cmi.core.lesson_status", status);
    this.commit();
  };

  // Whether the lesson needs nothing more.
  Tracker.prototype.done = function () {
    return this.status === "completed" || this.status === "passed";
  };

  // The way taken to the bookmarked slide (the walkthrough's data-trail,
  // slide indices), saved by `visit`; "" if none or not valid.
  Tracker.prototype.trail = function () {
    var trail = this.get("cmi.suspend_data");
    return /^\d+( \d+)*$/.test(trail) ? trail : "";
  };

  // A slide was shown: `bookmark` is its id; `end` whether it ends the
  // walkthrough (data-end). `trail`, the way taken (data-trail), is kept
  // in cmi.suspend_data, its latest part within SCORM 1.2's 4096
  // characters, so Back retraces it after a resume.
  Tracker.prototype.visit = function (bookmark, end, trail) {
    this.set("cmi.core.lesson_location", bookmark);
    if (typeof trail === "string") {
      if (trail.length > MAX_SUSPEND_DATA) {
        var cut = trail.indexOf(" ", trail.length - MAX_SUSPEND_DATA - 1);
        trail = cut < 0 ? "" : trail.slice(cut + 1);
      }
      this.set("cmi.suspend_data", trail);
    }
    if (end && this.completion === "last-step" && !this.done()) this.setStatus("completed");
  };

  // The quiz was answered with `right` of `total` correct. Reports the score
  // (0 to 100) and, by the quiz rule, passed or failed; a pass is not undone
  // by a worse try. Returns the score and whether it reaches the mastery
  // score.
  Tracker.prototype.answer = function (right, total) {
    var score = total > 0 ? Math.round((right / total) * 100) : 0;
    var passed = score >= this.mastery;
    if (passed || this.status !== "passed") {
      this.set("cmi.core.score.min", 0);
      this.set("cmi.core.score.max", 100);
      this.set("cmi.core.score.raw", score);
    }
    if (this.completion === "quiz-passed" && this.status !== "passed") {
      this.setStatus(passed ? "passed" : "failed");
    }
    this.commit();
    return { score: score, passed: passed };
  };

  // Ends the session (once): its time, whether to resume next time, then
  // LMSFinish.
  Tracker.prototype.finish = function (ms) {
    if (!this.api || this.finished) return;
    this.finished = true;
    this.set("cmi.core.session_time", sessionTime(ms));
    this.set("cmi.core.exit", this.done() ? "" : "suspend");
    this.commit();
    try {
      this.api.LMSFinish("");
    } catch (e) {
      // Nothing more to do.
    }
  };

  // Wires a tracker to the page: slides shown, the quiz, and leaving.
  function start(win, doc) {
    var root = doc.getElementById("wt");
    var sco = doc.getElementById("scorm");
    if (!root || !sco) return;
    var began = Date.now();
    var tracker = new Tracker(
      findApi(win),
      sco.getAttribute("data-completion"),
      Number(sco.getAttribute("data-mastery") || "100")
    );
    var slides = Array.prototype.slice.call(root.querySelectorAll(".wt-slide"));

    // Back where the learner left off, before the walkthrough reads the hash.
    var bookmark = tracker.begin();
    if (/^wt-slide-\d+$/.test(bookmark) && !win.location.hash && doc.getElementById(bookmark)) {
      try {
        win.history.replaceState(null, "", "#" + bookmark);
        root.setAttribute("data-trail", tracker.trail());
      } catch (e) {
        // Not allowed here; the walkthrough starts at the beginning.
      }
    }

    // The walkthrough shows one slide and hides the others.
    function shown() {
      var visible = slides.filter(function (slide) {
        return !slide.hidden;
      });
      if (visible.length !== 1) return;
      tracker.visit(visible[0].id, visible[0].hasAttribute("data-end"), root.getAttribute("data-trail") || "");
    }
    if (typeof win.MutationObserver === "function") {
      new win.MutationObserver(shown).observe(root, { subtree: true, attributes: true, attributeFilter: ["hidden"] });
    }

    var quiz = doc.getElementById("scorm-quiz");
    if (quiz) {
      var result = quiz.querySelector(".scorm-result");
      var answers = (quiz.getAttribute("data-answers") || "").split(" ").filter(Boolean);
      // Arrow keys move between choices here, not between slides.
      quiz.addEventListener("keydown", function (event) {
        if (event.key === "ArrowLeft" || event.key === "ArrowRight") event.stopPropagation();
      });
      quiz.addEventListener("submit", function (event) {
        event.preventDefault();
        var right = 0;
        var missing = 0;
        answers.forEach(function (answer, i) {
          var picked = quiz.querySelector('input[name="scorm-q' + (i + 1) + '"]:checked');
          if (!picked) missing++;
          else if (picked.value === answer) right++;
        });
        var message;
        if (missing) {
          message = quiz.getAttribute("data-unanswered") || "";
        } else {
          var outcome = tracker.answer(right, answers.length);
          message = (quiz.getAttribute(outcome.passed ? "data-passed" : "data-failed") || "")
            .split("{score}")
            .join(String(outcome.score));
        }
        result.textContent = message;
      });
    }

    function leave() {
      tracker.finish(Date.now() - began);
    }
    win.addEventListener("pagehide", leave);
    win.addEventListener("beforeunload", leave);
  }

  return { findApi: findApi, sessionTime: sessionTime, Tracker: Tracker, start: start };
});
