"use strict";
// Wordle policy-tree viewer.
//
// Loads a *readable* policy tree (the self-contained JSON produced by
// `wordle-opt solve --format readable`), validates it, and offers two views:
//
//   - Play: traverse the policy like the game. The current guess is shown, you
//     enter the response you would get, and it either advances, says the
//     response is impossible, or reports a solve. Back/Restart included.
//   - Explore tree: the full collapsible tree.
//
// The validation is deliberately the same rule the Rust validator enforces: at
// every node, the response edges must equal exactly the responses possible for
// some still-reachable candidate, each edge must lead to precisely those
// candidates, a leaf must be a win, and every candidate must terminate.

// The response/win string length is derived from the tree's words, so the
// viewer is not tied to 5 letters. `n` is the word length.
function winString(n) {
  return "g".repeat(n);
}

// --- response computation (same two-pass rule as Rust `Response::compute`,
// generalised to any word length) ---
function response(secret, guess) {
  const n = secret.length;
  const r = new Array(n).fill(0);
  const used = new Array(n).fill(false);
  for (let i = 0; i < n; i++) if (guess[i] === secret[i]) { r[i] = 1; used[i] = true; }
  for (let i = 0; i < n; i++) {
    if (r[i] === 1) continue;
    for (let j = 0; j < n; j++) {
      if (guess[i] === secret[j] && !used[j]) { r[i] = 2; used[j] = true; break; }
    }
  }
  let s = "";
  for (let i = 0; i < n; i++) s += r[i] === 0 ? "b" : r[i] === 1 ? "g" : "y";
  return s;
}

// Numeric ordering of a response string (little-endian base 3), matching the
// compact format's edge order. Length-agnostic.
function responseIndex(s) {
  let mul = 1, v = 0;
  for (let i = 0; i < s.length; i++) {
    v += (s[i] === "b" ? 0 : s[i] === "g" ? 1 : 2) * mul;
    mul *= 3;
  }
  return v;
}

function wordLength(file) {
  const sample = (file.candidates && file.candidates[0]) || (file.guesses && file.guesses[0]) || file.root.guess;
  return sample.length;
}

// --- validation (shared by both views; drives the stats + badge) ---
function validateTree(file) {
  const errors = [];
  const WIN = winString(wordLength(file));
  const guesses = new Set(file.guesses);
  const nCandidates = file.candidates.length;
  let nodes = 0, edges = 0, leaves = 0, maxDepth = 0, wins = 0, totalCost = 0;
  const depthHistogram = [];
  const stack = [[file.root, 0, file.candidates.map((_, i) => i)]];

  while (stack.length) {
    const [node, depth, candidates] = stack.pop();
    nodes++;
    edges += Object.keys(node.children).length;
    maxDepth = Math.max(maxDepth, depth);
    if (!guesses.has(node.guess)) { errors.push(`depth ${depth}: guess "${node.guess}" is not a valid guess`); continue; }
    if (candidates.length === 0) { errors.push(`depth ${depth}: empty candidate set`); continue; }

    const buckets = new Map();
    for (const c of candidates) {
      const r = response(file.candidates[c], node.guess);
      if (!buckets.has(r)) buckets.set(r, []);
      buckets.get(r).push(c);
    }
    const possible = [...buckets.keys()].filter((r) => r !== WIN).sort();
    const actual = Object.keys(node.children).sort();
    if (possible.length !== actual.length || possible.some((r, i) => r !== actual[i])) {
      const missing = possible.filter((r) => !actual.includes(r));
      const extra = actual.filter((r) => !possible.includes(r));
      errors.push(`depth ${depth} (guess "${node.guess}", ${candidates.length} candidates): missing [${missing}], impossible/duplicate [${extra}]`);
      if (errors.length > 25) break;
    }

    const winsHere = (buckets.get(WIN) || []).length;
    wins += winsHere;
    if (winsHere > 0) {
      totalCost += winsHere * (depth + 1);
      depthHistogram[depth] = (depthHistogram[depth] || 0) + winsHere;
    }
    const childKeys = Object.keys(node.children);
    if (childKeys.length === 0) {
      leaves++;
      if (candidates.length !== 1 || winsHere !== 1) errors.push(`depth ${depth}: leaf is not a win`);
      continue;
    }
    if (candidates.length === 1) { errors.push(`depth ${depth}: node has edges but one candidate remains`); continue; }
    for (const r of childKeys) stack.push([node.children[r], depth + 1, buckets.get(r) || []]);
  }
  if (wins !== nCandidates) errors.push(`${wins} wins but ${nCandidates} candidates - not every candidate terminates`);

  return {
    ok: errors.length === 0,
    errors,
    stats: {
      nodes, edges, leaves, maxDepth, wins, totalCost,
      meanGuesses: nCandidates ? totalCost / nCandidates : 0,
      candidates: nCandidates, guesses: file.guesses.length, depthHistogram,
    },
  };
}

// --- tiny DOM helpers ---
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

function tiles(chars, kind, extraCls) {
  const wrap = el("span", "tiles " + (kind || ""));
  for (const ch of chars) {
    let cls = "tile";
    if (ch === "b") cls += " gray";
    else if (ch === "g") cls += " green";
    else if (ch === "y") cls += " yellow";
    else cls += " unset";
    if (extraCls) cls += " " + extraCls;
    wrap.appendChild(el("span", cls, ch && ch !== " " ? ch : ""));
  }
  return wrap;
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// --- app state ---
const app = {
  file: null,
  report: null,
  letters: 5,
  win: "ggggg",
  mode: "play",
  // play state
  play: null,
  // per-node stats (Map node -> {n, total, exp})
  nodeStats: new Map(),
  // tree state
  allNodes: [],
  expandDepth: 2,
  query: "",
};

function candidatesMap(file, node, candidates) {
  const m = new Map();
  for (const c of candidates) {
    const r = response(file.candidates[c], node.guess);
    if (!m.has(r)) m.set(r, []);
    m.get(r).push(c);
  }
  return m;
}

// Expected guesses remaining from each node, computed from the fully
// determined subtree: T(C) = |C| + sum over non-win children T(C_r), so the
// expected number of further guesses starting at a node is T(C)/|C| (this
// includes the node's own guess, and correctly credits the candidate the guess
// itself wins). This is the meaningful per-node metric - the raw number of
// out-edges understates it whenever the guess is also a candidate.
function computeNodeStats(file) {
  const stats = new Map();
  const WIN = winString(wordLength(file));
  function visit(node, candidates) {
    const map = candidatesMap(file, node, candidates);
    let total = candidates.length; // the node's own guess, once per candidate
    for (const [r, cs] of map) {
      if (r === WIN) continue;
      const child = node.children[r];
      if (child) total += visit(child, cs);
    }
    stats.set(node, { n: candidates.length, total, exp: total / candidates.length });
    return total;
  }
  visit(file.root, file.candidates.map((_, i) => i));
  return stats;
}

// --- play view ---
function resetPlay() {
  app.play = {
    nodes: [{ node: app.file.root, candidates: app.file.candidates.map((_, i) => i) }],
    responses: [],
    editor: new Array(app.letters).fill(null),
    solved: false,
  };
  renderPlay();
}

function currentPlay() {
  return app.play.nodes[app.play.nodes.length - 1];
}

function submitResponse() {
  const p = app.play;
  if (p.solved) return;
  if (p.editor.some((x) => x === null)) return;
  const r = p.editor.join("");
  const cur = currentPlay();
  const map = candidatesMap(app.file, cur.node, cur.candidates);

  if (!map.has(r)) {
    const possible = [...map.keys()].sort((a, b) => responseIndex(a) - responseIndex(b));
    setPlayMsg(`"${r.toUpperCase()}" is not possible here (${possible.length} possible response(s)).`, "bad");
    return;
  }
  if (r === app.win) {
    p.solved = true;
    p.responses.push(r);
    p.editor = new Array(app.letters).fill(null);
    setPlayMsg(`Solved in ${p.responses.length} guess${p.responses.length === 1 ? "" : "es"}.`, "ok");
    renderPlay();
    return;
  }
  const child = cur.node.children[r];
  p.responses.push(r);
  p.nodes.push({ node: child, candidates: map.get(r) });
  p.editor = new Array(app.letters).fill(null);
  setPlayMsg("");
  renderPlay();
}

function backPlay() {
  const p = app.play;
  if (p.responses.length === 0) return;
  p.responses.pop();
  p.nodes.pop();
  p.solved = false;
  p.editor = new Array(app.letters).fill(null);
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

  // Current row.
  const cur = currentPlay();
  const isLeaf = Object.keys(cur.node.children).length === 0;
  const row = el("div", "row");
  const showGreen = p.solved || isLeaf;
  for (let k = 0; k < app.letters; k++) {
    row.appendChild(el("span", "tile " + (showGreen ? "green" : "current"), cur.node.guess[k]));
  }
  board.appendChild(row);

  // Editor.
  const editor = document.getElementById("editor");
  const done = document.getElementById("play-done");
  if (p.solved || isLeaf) {
    editor.hidden = true;
    done.hidden = false;
    const n = p.solved ? p.responses.length : p.responses.length + 1;
    done.textContent = `\u2713 ${cur.node.guess.toUpperCase()} \u2014 solved in ${n} guess${n === 1 ? "" : "es"}`;
  } else {
    editor.hidden = false;
    done.hidden = true;
    document.getElementById("editor-guess").textContent = cur.node.guess.toUpperCase();
    const st = app.nodeStats.get(cur.node);
    document.getElementById("editor-exp").textContent = st
      ? `${st.exp.toFixed(2)} expected guesses remaining \u00b7 ${st.n} candidate${st.n === 1 ? "" : "s"}`
      : "";
    renderEditor();
  }
  document.getElementById("back").disabled = p.responses.length === 0;
  document.getElementById("editor-hint").textContent = "";
}

function renderEditor() {
  const p = app.play;
  const box = document.getElementById("response-tiles");
  box.textContent = "";
  for (let i = 0; i < app.letters; i++) {
    const ch = p.editor[i];
    const cls = "tile " + (ch ? { b: "gray", g: "green", y: "yellow" }[ch] : "unset");
    const t = el("span", cls, ch ? ch.toUpperCase() : "");
    t.title = "click to cycle: unset \u2192 gray \u2192 yellow \u2192 green";
    t.addEventListener("click", () => {
      const idx = CYCLE.indexOf(p.editor[i]);
      p.editor[i] = CYCLE[(idx + 1) % CYCLE.length];
      renderEditor();
    });
    box.appendChild(t);
  }
  document.getElementById("submit-response").disabled = p.editor.some((x) => x === null);
}

function showHint() {
  const cur = currentPlay();
  const map = candidatesMap(app.file, cur.node, cur.candidates);
  const keys = [...map.keys()].filter((r) => r !== app.win).sort((a, b) => responseIndex(a) - responseIndex(b));
  const hint = document.getElementById("editor-hint");
  hint.textContent = "";
  const winPossible = map.has(app.win);
  hint.appendChild(el("div", "", `${keys.length} possible response${keys.length === 1 ? "" : "s"}${winPossible ? " (or the win)" : ""} \u2014 click one:`));
  const list = el("div", "hint-list");
  list.style.display = "flex";
  list.style.flexWrap = "wrap";
  list.style.gap = "6px";
  list.style.justifyContent = "center";
  for (const r of keys) {
    const t = tiles(r, "", "");
    t.style.cursor = "pointer";
    t.title = "use this response";
    t.addEventListener("click", () => {
      app.play.editor = r.split("");
      renderEditor();
    });
    list.appendChild(t);
  }
  hint.appendChild(list);
}

// --- tree view ---
function buildNode(node, depth, edge, parentUl) {
  const li = el("li", "node-item");
  const row = el("div", "node");
  const childKeys = Object.keys(node.children).sort((a, b) => responseIndex(a) - responseIndex(b));
  const hasChildren = childKeys.length > 0;

  const toggle = el("button", "toggle", hasChildren ? "\u25B8" : "\u00b7");
  toggle.disabled = !hasChildren;
  if (!hasChildren) toggle.classList.add("leaf");
  row.appendChild(toggle);
  row.appendChild(edge === null ? el("span", "edge root", "start") : tiles(edge, "edge"));
  row.appendChild(tiles(node.guess, "letter"));
  const st = app.nodeStats.get(node);
  const meta = st ? `${st.exp.toFixed(2)} exp \u00b7 ${st.n} cand` : hasChildren ? "branch" : "win";
  row.appendChild(el("span", "meta", meta));
  li.appendChild(row);

  const ul = el("ul", "children");
  ul.hidden = true;
  li.appendChild(ul);
  const entry = { li, ul, depth, childKeys, rendered: false, guess: node.guess };
  app.allNodes.push(entry);
  li._entry = entry;

  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!entry.rendered) {
      for (const r of entry.childKeys) buildNode(node.children[r], depth + 1, r, ul);
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
  buildNode(app.file.root, 0, null, rootUl);
  main.appendChild(rootUl);
  applySearch();
}

function setAll(expand) {
  for (const entry of [...app.allNodes]) {
    if (expand && !entry.rendered && entry.childKeys.length) entry.li.querySelector(".toggle").click();
  }
  for (const entry of app.allNodes) {
    if (!entry.childKeys.length) continue;
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

function showStats(file, report) {
  const box = document.getElementById("stats");
  box.hidden = false;
  box.textContent = "";
  const s = report.stats;
  const items = [
    ["strategy", file.strategy], ["candidates", s.candidates.toLocaleString()],
    ["nodes", s.nodes.toLocaleString()], ["edges", s.edges.toLocaleString()],
    ["max depth", s.maxDepth], ["mean guesses", s.meanGuesses.toFixed(4)],
    ["dictionary hash", file.dictionary_hash],
  ];
  for (const [k, v] of items) {
    const d = el("div", "stat");
    d.appendChild(el("span", "k", k));
    d.appendChild(el("span", "v", String(v)));
    box.appendChild(d);
  }
  const badge = el("div", "stat badge " + (report.ok ? "ok" : "bad"));
  badge.appendChild(el("span", "k", "validation"));
  badge.appendChild(el("span", "v", report.ok ? "\u2713 valid" : "\u2717 " + report.errors.length + " problem(s)"));
  box.appendChild(badge);
  const status = document.getElementById("status");
  status.className = "status " + (report.ok ? "ok" : "bad");
  status.textContent = report.ok
    ? `Validated ${s.nodes.toLocaleString()} nodes / ${s.candidates.toLocaleString()} candidates \u2014 every edge corresponds to a possible response.`
    : "INVALID \u2014 " + report.errors[0];
}

function setMode(mode) {
  app.mode = mode;
  document.getElementById("mode-play").classList.toggle("active", mode === "play");
  document.getElementById("mode-tree").classList.toggle("active", mode === "tree");
  document.getElementById("play").hidden = mode !== "play";
  document.getElementById("tree-pane").hidden = mode !== "tree";
  if (mode === "tree") renderTree();
}

function loadFile(file) {
  const status = document.getElementById("status");
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
  app.file = file;
  app.letters = wordLength(file);
  app.win = winString(app.letters);
  const report = validateTree(file);
  app.report = report;
  app.nodeStats = computeNodeStats(file);
  document.getElementById("depth").max = String(Math.max(1, report.stats.maxDepth));
  showStats(file, report);
  resetPlay();
  setMode("play");
  document.getElementById("tree").textContent = "";
  app.allNodes = [];
}

// Load examples by injecting a <script> that defines the tree. This works both
// over HTTP and when index.html is opened directly from disk, where fetch() of
// a sibling file is blocked by the browser.
function loadExample(name) {
  const status = document.getElementById("status");
  status.className = "status";
  status.textContent = `Loading ${name}\u2026`;
  const existing = window.WordleExamples && window.WordleExamples[name];
  if (existing) return loadFile(existing);
  const s = document.createElement("script");
  s.src = `examples/${name}.js`;
  s.onload = () => {
    const f = window.WordleExamples && window.WordleExamples[name];
    if (f) loadFile(f);
    else { status.className = "status bad"; status.textContent = `examples/${name}.js did not define a tree.`; }
  };
  s.onerror = () => {
    status.className = "status bad";
    status.innerHTML =
      `Could not load examples/${name}.js.<br>If you opened this file directly from disk, ` +
      `run a local server in the <code>site</code> folder (e.g. <code>python3 -m http.server</code>) ` +
      `or use <strong>Load tree&hellip;</strong> to open the file yourself.`;
  };
  document.head.appendChild(s);
}

// --- wiring ---
document.querySelectorAll("button.example").forEach((b) =>
  b.addEventListener("click", () => loadExample(b.dataset.example))
);
document.getElementById("mode-play").addEventListener("click", () => setMode("play"));
document.getElementById("mode-tree").addEventListener("click", () => setMode("tree"));

document.getElementById("file").addEventListener("change", (e) => {
  const f = e.target.files[0];
  if (!f) return;
  const reader = new FileReader();
  reader.onload = () => {
    try { loadFile(JSON.parse(reader.result)); }
    catch (err) {
      const status = document.getElementById("status");
      status.className = "status bad";
      status.textContent = "Could not parse JSON: " + err.message;
    }
  };
  reader.readAsText(f);
});

document.getElementById("submit-response").addEventListener("click", submitResponse);
document.getElementById("clear-response").addEventListener("click", () => { app.play.editor = new Array(app.letters).fill(null); renderEditor(); });
document.getElementById("hint-response").addEventListener("click", showHint);
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

loadExample("optimal");
