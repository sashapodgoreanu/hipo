/* ==========================================================================
   Learn Duckle - fluid editorial scroll
   Scroll-reveal (IntersectionObserver), top progress bar, chapter dots,
   and the interactive figures (canvas / compile / quiz / codebase map).
   Progressive enhancement: with no JS everything is visible, plain scroll.
   ========================================================================== */
(function () {
  var body = document.body;
  body.classList.add("js");

  var reduce = false;
  try { reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches; } catch (e) {}

  /* ---- stagger delays for reveals within each section ---- */
  [].forEach.call(document.querySelectorAll(".act"), function (sec) {
    [].forEach.call(sec.querySelectorAll(".reveal"), function (el, i) {
      el.style.transitionDelay = Math.min(i * 0.07, 0.45) + "s";
    });
  });

  /* ---- reveal on scroll ---- */
  var revealables = [].slice.call(document.querySelectorAll(".reveal"));
  if ("IntersectionObserver" in window && !reduce) {
    var ro = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { e.target.classList.add("in"); ro.unobserve(e.target); }
      });
    }, { rootMargin: "0px 0px -12% 0px", threshold: 0.12 });
    revealables.forEach(function (el) { ro.observe(el); });
  } else {
    revealables.forEach(function (el) { el.classList.add("in"); });
  }

  /* ---- top progress bar ---- */
  var bar = document.getElementById("lxBar");
  function onScroll() {
    var h = document.documentElement.scrollHeight - window.innerHeight;
    var p = h > 0 ? window.scrollY / h : 0;
    if (bar) bar.style.width = (p * 100).toFixed(2) + "%";
  }
  var ticking = false;
  window.addEventListener("scroll", function () {
    if (!ticking) { window.requestAnimationFrame(function () { onScroll(); ticking = false; }); ticking = true; }
  }, { passive: true });
  onScroll();

  /* ---- chapter dots ---- */
  var sections = [].slice.call(document.querySelectorAll(".act[data-chapter]"));
  var dotsWrap = document.getElementById("lxDots");
  var dots = [];
  if (dotsWrap) {
    sections.forEach(function (sec, i) {
      var b = document.createElement("button");
      b.className = "lx-dot"; b.type = "button";
      b.setAttribute("aria-label", sec.getAttribute("data-chapter"));
      b.innerHTML = '<span class="lbl">' + sec.getAttribute("data-chapter") + '</span>';
      b.addEventListener("click", function () {
        sec.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "start" });
      });
      dotsWrap.appendChild(b); dots.push(b);
    });
    if ("IntersectionObserver" in window) {
      var so = new IntersectionObserver(function (entries) {
        entries.forEach(function (e) {
          if (e.isIntersecting) {
            var idx = sections.indexOf(e.target);
            dots.forEach(function (d, i) { d.classList.toggle("on", i === idx); });
          }
        });
      }, { rootMargin: "-45% 0px -45% 0px", threshold: 0 });
      sections.forEach(function (s) { so.observe(s); });
    }
  }

  /* ===================== interactive figures ===================== */
  function initCanvas(el) {
    var nodes = [].slice.call(el.querySelectorAll(".node"));
    var wires = [].slice.call(el.querySelectorAll(".wire"));
    var cap = el.querySelector(".canvas-cap");
    var timers = [];
    function clear() { timers.forEach(clearTimeout); timers = []; }
    function play() {
      clear();
      nodes.forEach(function (n) { n.classList.remove("pop"); });
      wires.forEach(function (w) { w.classList.remove("lit"); });
      if (cap) cap.innerHTML = "";
      var t = 260;
      nodes.forEach(function (n, idx) {
        timers.push(setTimeout(function () {
          n.classList.add("pop");
          if (idx > 0 && wires[idx - 1]) wires[idx - 1].classList.add("lit");
          if (cap) cap.innerHTML = n.getAttribute("data-cap") || "";
        }, t));
        t += 950;
      });
      var done = el.getAttribute("data-done");
      if (done) timers.push(setTimeout(function () { if (cap) cap.innerHTML = done; }, t));
    }
    el._play = play;
    var replay = el.querySelector('[data-act="replay"]');
    if (replay) replay.addEventListener("click", function (e) { e.preventDefault(); play(); });
    // play once when it scrolls into view
    if ("IntersectionObserver" in window && !reduce) {
      var seen = false;
      var o = new IntersectionObserver(function (entries) {
        entries.forEach(function (e) { if (e.isIntersecting && !seen) { seen = true; play(); } });
      }, { threshold: 0.4 });
      o.observe(el);
    } else {
      nodes.forEach(function (n) { n.classList.add("pop"); });
      wires.forEach(function (w) { w.classList.add("lit"); });
      if (cap && nodes.length) cap.innerHTML = el.getAttribute("data-done") || "";
    }
  }

  function initCompile(el) {
    var btn = el.querySelector('[data-act="compile"]');
    var box = el.querySelector(".compile");
    if (!btn || !box) return;
    btn.addEventListener("click", function () {
      var open = box.classList.toggle("show");
      var lbl = btn.querySelector(".lbl");
      if (lbl) lbl.textContent = open ? "Hide the SQL" : "Compile to DuckDB SQL";
    });
  }

  function initQuiz(el) {
    var opts = [].slice.call(el.querySelectorAll(".opt"));
    var explain = el.querySelector(".explain");
    var done = false;
    opts.forEach(function (o) {
      o.addEventListener("click", function () {
        if (done) return; done = true;
        opts.forEach(function (x) { if (x.getAttribute("data-correct")) x.classList.add("right"); });
        if (!o.getAttribute("data-correct")) o.classList.add("wrong");
        if (explain) explain.classList.add("show");
      });
    });
  }

  var CRATES = {
    engine: { lang: "Rust crate", name: "crates/duckdb-engine", body: "The heart. It turns a pipeline graph into a plan of DuckDB SQL stages and executes them. Sub-modules handle planning, the per-component SQL builders, external-driver connectors, the executor, and merged TLS roots. Every other surface calls into this one crate, so a pipeline runs identically everywhere.", files: ["plan/mod.rs", "plan/builders.rs", "plan/graph.rs", "connectors.rs", "executor", "tls.rs"] },
    metadata: { lang: "Rust crate", name: "crates/metadata", body: "The shared vocabulary. Defines the PipelineNode, NodeData, Schema and edge types that the engine, runner, MCP server and desktop back end all speak. One source of truth for what a pipeline is on disk.", files: ["PipelineNode", "NodeData", "Schema", "Edge"] },
    runner: { lang: "Rust binary", name: "crates/duckle-runner", body: "The headless CLI. Runs a pipeline .json with no GUI - perfect for cron, systemd or CI. Its serve mode hosts a small web console with Operations, Pipelines, Runs history and a built-in interval + cron scheduler, all from one process.", files: ["main.rs", "serve.rs", "panel.html"] },
    mcp: { lang: "Rust binary", name: "crates/duckle-mcp", body: "The LLM bridge. A stdio MCP server that lets Claude or any client list components, fetch a schema, create a validated pipeline, run it, read logs, and even build a standalone binary - plus lineage, verify and trust review tools.", files: ["main.rs", "catalog.json", "tools"] },
    lance: { lang: "Rust sidecar", name: "crates/duckle-lance", body: "The vector sidecar. Owns both LanceDB and Vortex behind a Parquet bridge, isolating their heavier Arrow / DataFusion / protoc dependencies from the main engine so the core stays lean.", files: ["lancedb", "vortex", "parquet-bridge"] },
    desktop: { lang: "Tauri app", name: "apps/desktop", body: "The studio shell. A Tauri (Rust) back end that hosts the canvas, manages the workspace, encrypts connection secrets at rest, runs Duckie (local Qwen via llama.cpp), and bridges the front end to the engine and MCP.", files: ["main.rs", "secrets.rs", "workspace_git.rs", "llama"] },
    frontend: { lang: "React + Vite", name: "frontend", body: "The canvas you draw on. A React front end with the node graph, the component palette, the Visual Mapper, the properties panel, live preview, and the Runs / Console / Plan tabs. Compiled and embedded so the desktop app and duckle serve share the exact same UI.", files: ["App.tsx", "workflow-ui", "PropertiesPanel", "component-manifests"] }
  };
  function initCbmap(el) {
    var btns = [].slice.call(el.querySelectorAll(".crate-btn"));
    var detail = el.querySelector(".crate-detail");
    function show(key, btn) {
      var c = CRATES[key]; if (!c) return;
      btns.forEach(function (b) { b.classList.toggle("on", b === btn); });
      detail.innerHTML = '<div class="lang">' + c.lang + '</div><h4>' + c.name + '</h4><p>' + c.body + '</p><div class="files">' + c.files.map(function (f) { return "<code>" + f + "</code>"; }).join("") + '</div>';
    }
    btns.forEach(function (b) { b.addEventListener("click", function () { show(b.getAttribute("data-crate"), b); }); });
    if (btns[0]) show(btns[0].getAttribute("data-crate"), btns[0]);
  }

  [].forEach.call(document.querySelectorAll('[data-widget="canvas"]'), initCanvas);
  [].forEach.call(document.querySelectorAll('[data-widget="compile"]'), initCompile);
  [].forEach.call(document.querySelectorAll('[data-widget="quiz"]'), initQuiz);
  [].forEach.call(document.querySelectorAll('[data-widget="cbmap"]'), initCbmap);
})();
