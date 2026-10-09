"use strict";
// Wordle policy-tree viewer.
//
// Loads a *readable* policy tree (the self-contained JSON produced by
// `wordle-opt solve`), validates it, and offers two views:
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

// Response colours cycled on click: gray -> yellow -> green -> gray. Tiles
// start gray (all black) so no clicks are wasted setting the common case.
const CYCLE = ["b", "y", "g"];

// --- validation (shared by both views; a tree is rejected on load if invalid) ---
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
    // `hasWin` is true when the node's own guess is one of its candidates, i.e.
    // the all-green response is possible here.
    stats.set(node, { n: candidates.length, total, exp: total / candidates.length, hasWin: map.has(WIN) });
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
    editor: new Array(app.letters).fill("b"),
    userWin: false,
  };
  renderPlay();
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
    // The current guess is the answer. Do not push a response: the guess is
    // already on the board, so renderPlay just colours it green.
    p.userWin = true;
    setPlayMsg("");
    renderPlay();
    return;
  }
  const child = cur.node.children[r];
  p.responses.push(r);
  p.nodes.push({ node: child, candidates: map.get(r) });
  p.editor = new Array(app.letters).fill("b");
  setPlayMsg("");
  renderPlay();
}

function backPlay() {
  const p = app.play;
  if (p.userWin) {
    p.userWin = false;
    p.editor = new Array(app.letters).fill("b");
    setPlayMsg("");
    renderPlay();
    return;
  }
  if (p.responses.length === 0) return;
  p.responses.pop();
  p.nodes.pop();
  p.editor = new Array(app.letters).fill("b");
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
  // gray and cycles gray -> yellow -> green on click.
  const cur = currentPlay();
  const isLeaf = Object.keys(cur.node.children).length === 0;
  const solved = p.userWin || isLeaf;
  const row = el("div", "row");
  for (let k = 0; k < app.letters; k++) {
    const ch = p.editor[k];
    const cls = solved
      ? "tile green"
      : "tile clickable " + { b: "gray", g: "green", y: "yellow" }[ch];
    const t = el("span", cls, cur.node.guess[k]);
    if (!solved) {
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

  // Editor (buttons only - the response is set on the guess above).
  const editor = document.getElementById("editor");
  const done = document.getElementById("play-done");
  if (solved) {
    editor.hidden = true;
    done.hidden = false;
    const n = p.responses.length + 1;
    done.textContent = `\u2713 ${cur.node.guess.toUpperCase()}, solved in ${n} guess${n === 1 ? "" : "es"}`;
  } else {
    editor.hidden = false;
    done.hidden = true;
    const st = app.nodeStats.get(cur.node);
    document.getElementById("editor-exp").textContent = st
      ? `${st.exp.toFixed(2)} expected guesses remaining \u00b7 ${st.n} candidate${st.n === 1 ? "" : "s"}`
      : "";
  }
  document.getElementById("back").disabled = p.responses.length === 0 && !p.userWin;
  const opts = document.getElementById("editor-options");
  opts.hidden = true;
  opts.textContent = "";
  document.getElementById("options-response").setAttribute("aria-expanded", "false");
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
  const keys = [...map.keys()].filter((r) => r !== app.win).sort((a, b) => responseIndex(a) - responseIndex(b));
  box.textContent = "";
  const winPossible = map.has(app.win);
  box.appendChild(el("div", "", `${keys.length} possible response${keys.length === 1 ? "" : "s"}${winPossible ? " (or the win)" : ""}. Click one:`));
  const list = el("div", "options-list");
  for (const r of keys) {
    const row = el("div", "option-row");
    const t = tiles(r, "", "");
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
  const table = document.getElementById("stats-table");
  table.textContent = "";
  const thead = el("thead");
  const hr = el("tr");
  hr.appendChild(el("th", "", "Input"));
  hr.appendChild(el("th", "", "Policy"));
  thead.appendChild(hr);
  table.appendChild(thead);

  const inputCol = el("td");
  for (const [k, v] of [
    ["candidates", s.candidates.toLocaleString()],
    ["guesses", s.guesses.toLocaleString()],
    ["dictionary hash", file.dictionary_hash],
  ]) {
    inputCol.appendChild(el("div", "", `${k}: ${v}`));
  }
  const policyCol = el("td");
  for (const [k, v] of [
    ["name", file.strategy],
    ["nodes", s.nodes.toLocaleString()],
    ["edges", s.edges.toLocaleString()],
    ["max depth", String(s.maxDepth)],
    ["mean guesses", s.meanGuesses.toFixed(4)],
  ]) {
    policyCol.appendChild(el("div", "", `${k}: ${v}`));
  }
  const tr = el("tr");
  tr.appendChild(inputCol);
  tr.appendChild(policyCol);
  const tbody = el("tbody");
  tbody.appendChild(tr);
  table.appendChild(tbody);
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
  const bars = el("div", "chart-bars");
  for (const [guesses, count] of counts) {
    const col = el("div", "chart-col");
    col.appendChild(el("span", "count", count.toLocaleString()));
    const bar = el("div", "bar");
    bar.style.height = Math.max(1, Math.round((count / maxCount) * 130)) + "px";
    col.appendChild(bar);
    col.appendChild(el("span", "label", String(guesses)));
    bars.appendChild(col);
  }
  box.appendChild(bars);
}

function setMode(mode) {
  app.mode = mode;
  document.getElementById("mode-play").classList.toggle("active", mode === "play");
  document.getElementById("mode-tree").classList.toggle("active", mode === "tree");
  const selected = app.selected !== null;
  document.getElementById("play").hidden = !selected || mode !== "play";
  document.getElementById("tree-pane").hidden = !selected || mode !== "tree";
  if (mode === "tree" && selected) renderTree();
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

// Default to the optimal example.
loadExample("optimal");
