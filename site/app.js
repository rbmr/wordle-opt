"use strict";
// Wordle policy-tree viewer.
//
// Loads a *readable* policy tree (the self-contained JSON produced by
// `wordle-opt solve`), validates it, and offers two views:
//
//   - Play: traverse the policy like the game. The current guess is shown, you
//     set its response, and it either advances, says the response is
//     impossible, or reports a solve. Back/Restart included.
//   - Explore: the statistics and the full collapsible tree.
//
// The validation is deliberately the same rule the Rust validator enforces: at
// every node, the response edges must equal exactly the responses possible for
// some still-reachable candidate, each edge must lead to precisely those
// candidates, a leaf must be a win, and every candidate must terminate.
//
// That logic - response computation, candidate filtering, validation, per-node
// stats including the win rule - lives in `policy-core.js`, which is loaded
// before this file and shared with `policy-core.test.js`. This file is only the
// DOM/rendering layer.

const { winString, responseIndex, wordLength, candidatesMap, computeNodeStats, validateTree } = PolicyCore;

// Response colours cycled on click: gray -> yellow -> green -> gray. Tiles
// start gray (all black) so no clicks are wasted setting the common case.
const CYCLE = ["b", "y", "g"];

// --- tiny DOM helpers ---
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

// --- app state ---
const app = {
  file: null,
  report: null,
  selected: null,
  customName: "",
  customTree: null,
  letters: 5,
  win: "ggggg",
  mode: "play",
  // play state
  play: null,
  // per-node stats (Map node -> {n, total, exp})
  nodeStats: new Map(),
  // tree state
  allNodes: [],
  expandDepth: 1, // collapsed by default (the slider sets this to depth+1)
  query: "",
  // word-list state (Explore): the words, plus which file they were built for
  wordData: [],
  wordCandCount: 0,
  wordListFile: null,
  wordMatches: [],
  wordRowH: 0,
  wordScroll: null,
  // candidate-column filter: "all" | "candidates" | "non"
  wordFilter: "all",
  // which file the tree DOM was built for, so re-entering Explore is instant
  treeFile: null,
};

// --- play view ---
function resetPlay() {
  app.play = {
    nodes: [{ node: app.file.root, candidates: app.file.candidates.map((_, i) => i) }],
    responses: [],
    editor: new Array(app.letters).fill("b"),
    userWin: false,
  };
  autoFillIfSingleCandidate();
  renderPlay();
}

// With one candidate left, the all-green response is the only possible one, so
// pre-fill it as a convenience. It is still just a response: the win happens
// only when Submit is clicked, and the letters stay clickable so an impossible
// response can still be tried (Submit then reports it as usual).
function autoFillIfSingleCandidate() {
  const p = app.play;
  if (currentPlay().candidates.length === 1) p.editor = app.win.split("");
}

function currentPlay() {
  return app.play.nodes[app.play.nodes.length - 1];
}

function submitResponse() {
  const p = app.play;
  if (p.userWin) return;
  const r = p.editor.join("");
  const cur = currentPlay();
  const map = candidatesMap(app.file, cur.node, cur.candidates);

  if (!map.has(r)) {
    const possible = [...map.keys()].sort((a, b) => responseIndex(a) - responseIndex(b));
    setPlayMsg(`"${r.toUpperCase()}" is not possible here (${possible.length} possible response(s)).`, "bad");
    return;
  }
  if (r === app.win) {
    // The all-green response wins: this is a state of its own, reached only by
    // submitting it. Do not push a response - the guess is already on the
    // board, so renderPlay colours it green and the solved line counts it.
    p.userWin = true;
    setPlayMsg("");
    renderPlay();
    // The confetti button now sits exactly where Submit was; fire from there.
    launchConfetti(document.getElementById("confetti"));
    return;
  }
  const child = cur.node.children[r];
  p.responses.push(r);
  p.nodes.push({ node: child, candidates: map.get(r) });
  p.editor = new Array(app.letters).fill("b");
  autoFillIfSingleCandidate();
  setPlayMsg("");
  renderPlay();
}

function backPlay() {
  const p = app.play;
  if (p.userWin) {
    // Leave the win state: keep the winning guess on the board with the
    // all-green response still filled in, so a typo can be corrected.
    p.userWin = false;
    p.editor = app.win.split("");
    setPlayMsg("");
    renderPlay();
    return;
  }
  if (p.responses.length === 0) return;
  // Restore the response that was submitted for this guess, so Back is an edit
  // of the previous answer rather than starting it over.
  const r = p.responses.pop();
  p.nodes.pop();
  p.editor = r.split("");
  setPlayMsg("");
  renderPlay();
}

function setPlayMsg(msg, cls) {
  const m = document.getElementById("play-msg");
  m.textContent = msg || "";
  m.className = "play-msg" + (cls ? " " + cls : "");
}

function renderPlay() {
  const p = app.play;
  const board = document.getElementById("board");
  board.textContent = "";

  // Completed rows: guess coloured by the response the player entered.
  for (let i = 0; i < p.responses.length; i++) {
    const row = el("div", "row");
    const g = p.nodes[i].node.guess;
    for (let k = 0; k < app.letters; k++) {
      row.appendChild(el("span", "tile " + { b: "gray", g: "green", y: "yellow" }[p.responses[i][k]], g[k]));
    }
    board.appendChild(row);
  }

  // Current row: the guess itself is the response selector. Each letter starts
  // gray and cycles gray -> yellow -> green on click. In the win state the row
  // is shown all green and the letters are no longer clickable.
  const cur = currentPlay();
  const won = p.userWin;
  const row = el("div", "row");
  for (let k = 0; k < app.letters; k++) {
    const ch = p.editor[k];
    const cls = won
      ? "tile green"
      : "tile clickable " + { b: "gray", g: "green", y: "yellow" }[ch];
    const t = el("span", cls, cur.node.guess[k]);
    if (!won) {
      t.title = "click to cycle the response: gray \u2192 yellow \u2192 green";
      t.addEventListener("click", () => {
        const idx = CYCLE.indexOf(p.editor[k]);
        p.editor[k] = CYCLE[(idx + 1) % CYCLE.length];
        renderPlay();
      });
    }
    row.appendChild(t);
  }
  board.appendChild(row);

  // Editor. Playing and won use the same two blocks (text, then buttons) with
  // the same dimensions, so the controls below never move and the confetti
  // button lands exactly where Submit was.
  const editor = document.getElementById("editor");
  const hint = document.getElementById("editor-hint");
  const exp = document.getElementById("editor-exp");
  const solved = document.getElementById("editor-solved");
  const clear = document.getElementById("clear-response");
  const submit = document.getElementById("submit-response");
  const options = document.getElementById("options-response");
  const confetti = document.getElementById("confetti");
  editor.hidden = false;
  if (won) {
    hint.hidden = true;
    exp.hidden = true;
    solved.hidden = false;
    const n = p.responses.length + 1;
    solved.textContent = `\u2713 ${cur.node.guess.toUpperCase()}, solved in ${n} guess${n === 1 ? "" : "es"}`;
    clear.hidden = true;
    submit.hidden = true;
    options.hidden = true;
    confetti.hidden = false;
  } else {
    hint.hidden = false;
    exp.hidden = false;
    solved.hidden = true;
    clear.hidden = false;
    submit.hidden = false;
    options.hidden = false;
    confetti.hidden = true;
    const st = app.nodeStats.get(cur.node);
    exp.textContent = st
      ? `${st.exp.toFixed(2)} expected guesses remaining \u00b7 ${st.n} candidate${st.n === 1 ? "" : "s"}`
      : "";
  }
  document.getElementById("back").disabled = p.responses.length === 0 && !p.userWin;
  const opts = document.getElementById("editor-options");
  opts.hidden = true;
  opts.textContent = "";
  options.setAttribute("aria-expanded", "false");
}

// --- confetti ---
// A tiny dependency-free burst, launched from the centre of `originEl` (the
// confetti button, which sits where Submit was). Clicking it repeatedly just
// spawns more bursts.
function launchConfetti(originEl) {
  const rect = originEl.getBoundingClientRect();
  const canvas = el("canvas", "confetti-canvas");
  canvas.width = window.innerWidth;
  canvas.height = window.innerHeight;
  document.body.appendChild(canvas);
  const ctx = canvas.getContext("2d");
  const colors = ["#e5484d", "#f5a524", "#46a758", "#3b82f6", "#a855f7", "#ec4899"];
  const cx = rect.left + rect.width / 2;
  const cy = rect.top + rect.height / 2;
  const parts = [];
  for (let i = 0; i < 150; i++) {
    const angle = -Math.PI / 2 + (Math.random() - 0.5) * 1.8;
    const speed = 5 + Math.random() * 10;
    parts.push({
      x: cx,
      y: cy,
      vx: Math.cos(angle) * speed,
      vy: Math.sin(angle) * speed,
      w: 3 + Math.random() * 2,
      h: 4 + Math.random() * 3,
      rot: Math.random() * Math.PI,
      vr: (Math.random() - 0.5) * 0.35,
      color: colors[(Math.random() * colors.length) | 0],
    });
  }
  let frames = 0;
  function frame() {
    frames++;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.globalAlpha = Math.max(0, 1 - frames / 160);
    let onScreen = 0;
    for (const q of parts) {
      q.vy += 0.3;
      q.vx *= 0.995;
      q.x += q.vx;
      q.y += q.vy;
      q.rot += q.vr;
      if (q.y < canvas.height + 40) onScreen++;
      ctx.save();
      ctx.translate(q.x, q.y);
      ctx.rotate(q.rot);
      ctx.fillStyle = q.color;
      ctx.fillRect(-q.w / 2, -q.h / 2, q.w, q.h);
      ctx.restore();
    }
    ctx.globalAlpha = 1;
    if (onScreen > 0 && frames < 200) requestAnimationFrame(frame);
    else canvas.remove();
  }
  requestAnimationFrame(frame);
}

function toggleOptions() {
  const box = document.getElementById("editor-options");
  const btn = document.getElementById("options-response");
  // Clicking again collapses it.
  if (!box.hidden) {
    box.hidden = true;
    btn.setAttribute("aria-expanded", "false");
    return;
  }
  const cur = currentPlay();
  const map = candidatesMap(app.file, cur.node, cur.candidates);
  // Every possible response, including the all-green one when the win is
  // available here: picking it just fills it in like any other response, and
  // Submit is still what actually wins.
  const keys = [...map.keys()].sort((a, b) => responseIndex(a) - responseIndex(b));
  box.textContent = "";
  box.appendChild(el("div", "", `${keys.length} possible response${keys.length === 1 ? "" : "s"}. Click one:`));
  const list = el("div", "options-list");
  for (const r of keys) {
    const row = el("div", "option-row");
    // Show the current guess coloured by each possible response, matching the
    // board and tree language (rather than a row of b/g/y tiles).
    const t = wordTiles(cur.node.guess, r);
    t.style.cursor = "pointer";
    t.title = "use this response";
    t.addEventListener("click", () => {
      app.play.editor = r.split("");
      renderPlay();
    });
    row.appendChild(t);
    list.appendChild(row);
  }
  box.appendChild(list);
  box.hidden = false;
  btn.setAttribute("aria-expanded", "true");
}

// --- tree view ---
//
// Each node displays the *previous* guess, coloured by the response that led
// to it (the edge), rather than the next guess. The root has no previous guess,
// so it shows its own (the first guess) uncoloured. A node's own guess is
// therefore readable from its children. When the node's guess is itself a
// candidate, the all-green response is possible and is shown as a "solved"
// child.

// A word rendered as tiles, coloured by `response` (a b/g/y string), or
// uncoloured when `response` is null.
function wordTiles(word, response) {
  const wrap = el("span", "tiles word");
  for (let k = 0; k < word.length; k++) {
    const cls = "tile" + (response ? " " + ({ b: "gray", g: "green", y: "yellow" }[response[k]] || "") : "");
    wrap.appendChild(el("span", cls, word[k]));
  }
  return wrap;
}

function buildSolved(guess, parentUl) {
  const li = el("li", "node-item solved");
  const row = el("div", "node");
  row.appendChild(el("span", "toggle leaf", "\u00b7"));
  row.appendChild(wordTiles(guess, winString(guess.length)));
  row.appendChild(el("span", "meta", "solved"));
  li.appendChild(row);
  parentUl.appendChild(li);
  const entry = { li, ul: null, depth: 0, childKeys: [], rendered: true, guess, hasChildren: false };
  app.allNodes.push(entry);
}

function buildNode(node, depth, parentGuess, edge, parentUl) {
  const li = el("li", "node-item");
  const row = el("div", "node");
  const st = app.nodeStats.get(node);
  const childKeys = Object.keys(node.children).sort((a, b) => responseIndex(a) - responseIndex(b));
  const hasWin = st ? st.hasWin : false;
  const hasChildren = childKeys.length > 0 || hasWin;

  const toggle = el("button", "toggle", hasChildren ? "\u25B8" : "\u00b7");
  toggle.disabled = !hasChildren;
  if (!hasChildren) toggle.classList.add("leaf");
  row.appendChild(toggle);

  // Root: show this node's own guess uncoloured. Otherwise: show the parent's
  // guess coloured by the response that reached this node.
  const word = edge === null ? node.guess : parentGuess;
  row.appendChild(wordTiles(word, edge));
  row.appendChild(el("span", "meta", st ? `${st.exp.toFixed(2)} exp \u00b7 ${st.n} cand` : "win"));
  li.appendChild(row);

  const ul = el("ul", "children");
  ul.hidden = true;
  li.appendChild(ul);
  const entry = { li, ul, depth, childKeys, rendered: false, guess: node.guess, hasChildren };
  app.allNodes.push(entry);
  li._entry = entry;

  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!entry.rendered) {
      for (const r of entry.childKeys) buildNode(node.children[r], depth + 1, node.guess, r, ul);
      if (hasWin) buildSolved(node.guess, ul);
      entry.rendered = true;
    }
    ul.hidden = !ul.hidden;
    toggle.textContent = ul.hidden ? "\u25B8" : "\u25BE";
  });
  parentUl.appendChild(li);
  if (hasChildren && depth + 1 < app.expandDepth) toggle.click();
}

function renderTree() {
  const main = document.getElementById("tree");
  main.textContent = "";
  app.allNodes = [];
  if (!app.file) return;
  const rootUl = el("ul", "children root-children");
  buildNode(app.file.root, 0, null, null, rootUl);
  main.appendChild(rootUl);
  applySearch();
}

function setAll(expand) {
  for (const entry of [...app.allNodes]) {
    if (expand && !entry.rendered && entry.hasChildren) entry.li.querySelector(".toggle").click();
  }
  for (const entry of app.allNodes) {
    if (!entry.hasChildren || !entry.ul) continue;
    const t = entry.li.querySelector(".toggle");
    if (expand && entry.ul.hidden) t.click();
    if (!expand && !entry.ul.hidden) t.click();
  }
}

function applySearch() {
  const q = app.query.trim().toLowerCase();
  let matches = 0;
  for (const entry of app.allNodes) {
    const hit = q.length > 0 && entry.guess === q;
    entry.li.classList.toggle("match", hit);
    if (hit) matches++;
  }
  const s = document.getElementById("search");
  s.classList.toggle("no-match", q.length > 0 && matches === 0);
}

function renderStats(file, report) {
  const s = report.stats;
  const list = document.getElementById("stats-list");
  list.textContent = "";
  const rows = [
    ["candidates", s.candidates.toLocaleString()],
    ["guesses", s.guesses.toLocaleString()],
    ["dictionary hash", file.dictionary_hash],
    ["name", file.strategy],
    ["nodes", s.nodes.toLocaleString()],
    ["edges", s.edges.toLocaleString()],
    ["max depth", String(s.maxDepth)],
    ["mean guesses", s.meanGuesses.toFixed(4)],
  ];
  for (const [k, v] of rows) {
    const d = el("div", "stat-line");
    d.appendChild(el("span", "sl-label", k));
    d.appendChild(el("span", "sl-value", v));
    list.appendChild(d);
  }
}

// Bar plot of how many candidates are solved in 1, 2, 3 ... guesses.
function renderChart() {
  const box = document.getElementById("chart");
  box.textContent = "";
  if (!app.report) return;
  const hist = app.report.stats.depthHistogram; // hist[d] = wins at tree depth d
  const counts = [];
  for (let d = 0; d < hist.length; d++) {
    // Always show the "1 guess" bar (depth 0), even at zero, so the axis
    // starts there.
    if (d === 0 || hist[d]) counts.push([d + 1, hist[d] || 0]);
  }
  if (counts.length === 0) return;
  const maxCount = Math.max(1, ...counts.map(([, c]) => c));
  box.appendChild(el("div", "chart-title", "Guess-count distribution"));
  const plot = el("div", "chart-plot");
  const bars = el("div", "chart-bars");
  const labels = el("div", "chart-labels");
  for (const [guesses, count] of counts) {
    const col = el("div", "chart-col");
    col.appendChild(el("span", "count", count.toLocaleString()));
    const bar = el("div", "bar");
    bar.style.height = Math.max(1, Math.round((count / maxCount) * 130)) + "px";
    col.appendChild(bar);
    bars.appendChild(col);
    labels.appendChild(el("div", "chart-label", String(guesses)));
  }
  plot.appendChild(bars);
  plot.appendChild(labels);
  box.appendChild(plot);
}

// --- word list (Explore) ---
// The data is built in one cheap pass; the rows are rendered in chunks on
// animation frames, so Explore opens immediately (the header, filter and table
// shell are static markup) and the list fills in over the next few hundred
// milliseconds instead of blocking the first paint on ~15k rows.
function renderWordList() {
  app.wordData = [];
  app.wordCandCount = 0;
  const file = app.file;
  if (!file) return;
  const candidates = new Set(file.candidates.map((w) => w.toLowerCase()));
  for (const w of file.guesses) {
    const isCand = candidates.has(w.toLowerCase());
    if (isCand) app.wordCandCount++;
    app.wordData.push({ word: w, lower: w.toLowerCase(), isCand });
  }
  syncWordHeader();
  applyWordFilter();
}

// The marks are drawn as inline SVG rather than text glyphs: the symbol fonts
// render U+2713/U+2714 and U+2715/U+2716 with wildly different weights, so a
// check and a cross that look balanced in one font look mismatched in another.
// A fixed stroke width makes them match everywhere.
function markSvg(isCandidate) {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(NS, "path");
  path.setAttribute("fill", "none");
  path.setAttribute("stroke", "currentColor");
  path.setAttribute("stroke-width", "3.2");
  path.setAttribute("stroke-linecap", "round");
  path.setAttribute("stroke-linejoin", "round");
  path.setAttribute("d", isCandidate ? "M4 10.5 L8.5 15 L16 5.5" : "M5.5 5.5 L14.5 14.5 M14.5 5.5 L5.5 14.5");
  svg.appendChild(path);
  return svg;
}

function wordRow(d) {
  const tr = document.createElement("tr");
  tr.appendChild(el("td", "word", d.word));
  const mark = el("td", d.isCand ? "mark yes" : "mark no");
  mark.appendChild(markSvg(d.isCand));
  mark.title = d.isCand ? "candidate: can be the final answer" : "guess only: never the final answer";
  tr.appendChild(mark);
  return tr;
}

// The words matching both filters. Computed from the data, never from the DOM,
// so the shown count is exact even while the rows are still streaming in.
function wordMatches() {
  const q = (document.getElementById("word-search").value || "").trim().toLowerCase();
  const mode = app.wordFilter;
  return (app.wordData || []).filter(
    (d) => (q === "" || d.lower.includes(q)) && (mode === "all" || (mode === "candidates") === d.isCand)
  );
}

// Renders only the rows in (and just around) the scroll viewport, with spacer
// rows standing in for the rest. That keeps the scrollbar honest and the list
// effectively complete the moment Explore opens, instead of laying out ~15k
// rows (which is what made the first paint slow).
function spacerRow(heightPx) {
  const tr = document.createElement("tr");
  tr.className = "spacer";
  const td = document.createElement("td");
  td.colSpan = 2;
  td.style.height = heightPx + "px";
  tr.appendChild(td);
  return tr;
}

function wordRowHeight() {
  if (app.wordRowH) return app.wordRowH;
  const rows = document.querySelectorAll("#word-rows tr:not(.spacer)");
  if (rows.length >= 2) app.wordRowH = rows[1].offsetTop - rows[0].offsetTop;
  else if (rows.length === 1) app.wordRowH = rows[0].offsetHeight;
  return app.wordRowH || 31;
}

function drawWordWindow() {
  const wrap = document.querySelector(".word-table-wrap");
  const tbody = document.getElementById("word-rows");
  const matches = app.wordMatches || [];
  const rowH = wordRowHeight();
  // The wrap's height is content-driven (a max-height, not a fixed height), so
  // before any rows exist clientHeight is only the header. Size the window for
  // the largest viewport the wrap can ever have, so the first render already
  // covers it instead of leaving the bottom of the box empty.
  const maxH = parseFloat(getComputedStyle(wrap).maxHeight) || 460;
  const viewH = Math.max(wrap.clientHeight, maxH);
  const first = Math.max(0, Math.floor(wrap.scrollTop / rowH) - 5);
  const count = Math.ceil(viewH / rowH) + 10;
  const last = Math.min(matches.length, first + count);
  tbody.textContent = "";
  if (first > 0) tbody.appendChild(spacerRow(first * rowH));
  for (let i = first; i < last; i++) tbody.appendChild(wordRow(matches[i]));
  if (last < matches.length) tbody.appendChild(spacerRow((matches.length - last) * rowH));
}

// The words matching the current filters are handed straight to the windowed
// renderer, so a filter change is instant no matter how many rows match.
function renderWordRows(matches) {
  app.wordMatches = matches;
  const wrap = document.querySelector(".word-table-wrap");
  if (wrap) wrap.scrollTop = 0;
  drawWordWindow();
}

// Substring filter (case-insensitive) combined with the Candidate-column
// filter. The count is the size of the intersection of the two.
function applyWordFilter() {
  const matches = wordMatches();
  renderWordRows(matches);
  const total = (app.wordData || []).length;
  const count = document.getElementById("word-count");
  if (document.getElementById("word-search").value.trim() === "" && app.wordFilter === "all") {
    count.textContent = `${total.toLocaleString()} words \u00b7 ${app.wordCandCount.toLocaleString()} candidates`;
  } else {
    const suffix =
      app.wordFilter === "all" ? "" : app.wordFilter === "candidates" ? " \u00b7 candidates only" : " \u00b7 non-candidates only";
    count.textContent = `${matches.length.toLocaleString()} of ${total.toLocaleString()} shown${suffix}`;
  }
}

function syncWordHeader() {
  const th = document.getElementById("word-cand-header");
  th.dataset.mode = app.wordFilter;
  th.title =
    app.wordFilter === "all"
      ? "click to filter: all words \u2192 candidates only \u2192 non-candidates only"
      : app.wordFilter === "candidates"
        ? "showing candidates only; click for non-candidates"
        : "showing non-candidates only; click to show all";
}

// Clicking the Candidate header cycles all -> candidates only -> non-candidates
// -> all, always combined with whatever the substring filter says.
function cycleWordFilter() {
  const modes = ["all", "candidates", "non"];
  app.wordFilter = modes[(modes.indexOf(app.wordFilter) + 1) % modes.length];
  syncWordHeader();
  applyWordFilter();
}

function setMode(mode) {
  app.mode = mode;
  document.getElementById("mode-play").classList.toggle("active", mode === "play");
  document.getElementById("mode-tree").classList.toggle("active", mode === "tree");
  const selected = app.selected !== null;
  document.getElementById("play").hidden = !selected || mode !== "play";
  document.getElementById("tree-pane").hidden = !selected || mode !== "tree";
  if (mode === "tree" && selected) {
    // Re-entering Explore reuses the already-built DOM (including whatever the
    // user expanded); it is only rebuilt when a different tree is loaded.
    if (app.treeFile !== app.file) {
      renderTree();
      app.treeFile = app.file;
    }
    if (app.wordListFile !== app.file) {
      renderWordList();
      app.wordListFile = app.file;
    }
  }
}

// Reflects the selected policy (button highlight), the optional custom-file
// button, and whether the play/explore controls are available at all.
function updateSelectionUI() {
  document.querySelectorAll("button.policy.example").forEach((b) =>
    b.classList.toggle("active", app.selected === b.dataset.example)
  );
  const custom = document.getElementById("custom-policy");
  custom.hidden = !app.customName;
  if (app.customName) custom.textContent = app.customName;
  custom.classList.toggle("active", app.selected === "custom");
  const selected = app.selected !== null;
  document.getElementById("mode-bar").hidden = !selected;
  document.getElementById("select-hint").hidden = selected;
  setMode(app.mode);
}

// Validates and, if valid, selects the policy. An invalid tree is rejected
// outright, so anything that is loaded can be assumed valid.
function loadFile(file, selection, customName) {
  const status = document.getElementById("status");
  status.hidden = false;
  if (file.format !== "wordle-policy-tree") {
    status.className = "status bad";
    status.textContent = `Not a readable policy tree (format = ${file.format || "missing"}).`;
    return;
  }
  if (file.version !== 1) {
    status.className = "status bad";
    status.textContent = `Unsupported tree version ${file.version}.`;
    return;
  }
  const report = validateTree(file);
  if (!report.ok) {
    status.className = "status bad";
    status.textContent = `Policy rejected: ${report.errors[0]}`;
    return;
  }
  app.file = file;
  app.selected = selection;
  app.report = report;
  app.letters = wordLength(file);
  app.win = winString(app.letters);
  app.nodeStats = computeNodeStats(file);
  if (customName !== undefined) {
    app.customName = customName;
    app.customTree = file;
  }
  document.getElementById("depth").max = String(Math.max(1, report.stats.maxDepth));
  renderStats(file, report);
  renderChart();
  resetPlay();
  document.getElementById("tree").textContent = "";
  app.allNodes = [];
  app.treeFile = null;
  app.wordListFile = null;
  app.wordFilter = "all";
  document.getElementById("word-search").value = "";
  status.hidden = true;
  status.textContent = "";
  updateSelectionUI();
}

// Load examples by injecting a <script> that defines the tree. This works both
// over HTTP and when index.html is opened directly from disk, where fetch() of
// a sibling file is blocked by the browser.
function loadExample(name) {
  const status = document.getElementById("status");
  status.hidden = false;
  status.className = "status";
  status.textContent = `Loading ${name}\u2026`;
  const existing = window.WordleExamples && window.WordleExamples[name];
  if (existing) return loadFile(existing, name);
  const s = document.createElement("script");
  s.src = `examples/${name}.js`;
  s.onload = () => {
    const f = window.WordleExamples && window.WordleExamples[name];
    if (f) loadFile(f, name);
    else { status.className = "status bad"; status.textContent = `examples/${name}.js did not define a tree.`; }
  };
  s.onerror = () => {
    status.hidden = false;
    status.className = "status bad";
    status.innerHTML =
      `Could not load examples/${name}.js.<br>If you opened this file directly from disk, ` +
      `run a local server in the <code>site</code> folder (e.g. <code>python3 -m http.server</code>) ` +
      `or use <strong>Load tree&hellip;</strong> to open the file yourself.`;
  };
  document.head.appendChild(s);
}

// --- wiring ---
document.querySelectorAll("button.policy.example").forEach((b) =>
  b.addEventListener("click", () => loadExample(b.dataset.example))
);
document.getElementById("custom-policy").addEventListener("click", () => {
  if (app.customTree) loadFile(app.customTree, "custom");
});
document.getElementById("mode-play").addEventListener("click", () => setMode("play"));
document.getElementById("mode-tree").addEventListener("click", () => setMode("tree"));

document.getElementById("file").addEventListener("change", (e) => {
  const f = e.target.files[0];
  if (!f) return;
  const reader = new FileReader();
  reader.onload = () => {
    try {
      const obj = JSON.parse(reader.result);
      loadFile(obj, "custom", f.name);
    } catch (err) {
      const status = document.getElementById("status");
      status.hidden = false;
      status.className = "status bad";
      status.textContent = "Could not parse JSON: " + err.message;
    }
  };
  reader.readAsText(f);
  e.target.value = ""; // allow selecting the same file again
});

document.getElementById("submit-response").addEventListener("click", submitResponse);
document.getElementById("clear-response").addEventListener("click", () => { app.play.editor = new Array(app.letters).fill("b"); renderPlay(); });
document.getElementById("options-response").addEventListener("click", toggleOptions);
document.getElementById("confetti").addEventListener("click", (e) => launchConfetti(e.currentTarget));
document.getElementById("back").addEventListener("click", backPlay);
document.getElementById("reset").addEventListener("click", resetPlay);

document.getElementById("expand-all").addEventListener("click", () => setAll(true));
document.getElementById("collapse-all").addEventListener("click", () => setAll(false));
const depthEl = document.getElementById("depth");
depthEl.addEventListener("input", () => {
  document.getElementById("depth-val").textContent = depthEl.value;
  app.expandDepth = Number(depthEl.value) + 1;
  renderTree();
});
document.getElementById("search").addEventListener("input", (e) => { app.query = e.target.value; applySearch(); });
document.getElementById("word-search").addEventListener("input", applyWordFilter);
document.getElementById("word-cand-header").addEventListener("click", cycleWordFilter);
// Scroll is the only thing that changes which word rows are rendered.
document.querySelector(".word-table-wrap").addEventListener("scroll", () => {
  if (app.wordScroll) return;
  app.wordScroll = requestAnimationFrame(() => {
    app.wordScroll = null;
    drawWordWindow();
  });
});

// Default to the optimal example.
loadExample("optimal");
