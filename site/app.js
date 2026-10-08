"use strict";
// Wordle policy-tree viewer.
//
// Loads a *readable* policy tree (the self-contained JSON produced by
// `wordle-opt solve --format readable`), validates it against the defining
// invariant, summarises it, and renders it as a collapsible tree.
//
// The validation implemented here is deliberately the same rule the Rust
// validator enforces: at every node, the set of response edges must equal
// exactly the set of responses possible for some still-reachable candidate,
// each edge must lead to the subtree for precisely those candidates, a leaf
// must be a win, and every candidate must terminate.

const COLORS = { g: "green", y: "yellow", b: "gray" };

// --- response computation (same two-pass rule as the Rust `Response::compute`) ---
function response(secret, guess) {
  const r = [0, 0, 0, 0, 0];
  const used = [false, false, false, false, false];
  for (let i = 0; i < 5; i++) {
    if (guess[i] === secret[i]) {
      r[i] = 1;
      used[i] = true;
    }
  }
  for (let i = 0; i < 5; i++) {
    if (r[i] === 1) continue;
    for (let j = 0; j < 5; j++) {
      if (guess[i] === secret[j] && !used[j]) {
        r[i] = 2;
        used[j] = true;
        break;
      }
    }
  }
  let s = "";
  for (let i = 0; i < 5; i++) s += r[i] === 0 ? "b" : r[i] === 1 ? "g" : "y";
  return s;
}

const WIN = "ggggg";

// Numeric ordering of a response string (little-endian base 3), matching the
// compact format's edge order.
function responseIndex(s) {
  let mul = 1, v = 0;
  for (let i = 0; i < 5; i++) {
    const d = s[i] === "b" ? 0 : s[i] === "g" ? 1 : 2;
    v += d * mul;
    mul *= 3;
  }
  return v;
}

// --- validation ---
function validateTree(file) {
  const errors = [];
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

    if (!guesses.has(node.guess)) {
      errors.push(`depth ${depth}: guess "${node.guess}" is not in the embedded guess list`);
      continue;
    }
    if (candidates.length === 0) {
      errors.push(`depth ${depth}: empty candidate set`);
      continue;
    }

    // Partition the candidate set by the response to this guess.
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
      errors.push(
        `depth ${depth} (guess "${node.guess}", ${candidates.length} candidates): ` +
          `missing edges [${missing.join(", ")}], impossible/duplicate edges [${extra.join(", ")}]`
      );
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
      if (candidates.length !== 1 || winsHere !== 1) {
        errors.push(`depth ${depth}: leaf is not a win (${candidates.length} candidate(s) remain)`);
      }
      continue;
    }
    if (candidates.length === 1) {
      errors.push(`depth ${depth}: node has edges but only one candidate remains`);
      continue;
    }
    for (const r of childKeys) {
      stack.push([node.children[r], depth + 1, buckets.get(r) || []]);
    }
  }

  if (wins !== nCandidates) {
    errors.push(`${wins} candidate wins but ${nCandidates} candidates - not every candidate terminates`);
  }

  return {
    ok: errors.length === 0,
    errors,
    stats: {
      nodes, edges, leaves, maxDepth, wins, totalCost,
      meanGuesses: nCandidates ? totalCost / nCandidates : 0,
      candidates: nCandidates,
      guesses: file.guesses.length,
      depthHistogram,
    },
  };
}

// --- rendering ---
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

function tileRow(str, kind) {
  const wrap = el("span", "tiles " + kind);
  for (const ch of str) {
    const t = el("span", "tile " + (kind === "letter" ? "letter" : COLORS[ch] || "gray"), ch);
    wrap.appendChild(t);
  }
  return wrap;
}

const app = {
  file: null,
  report: null,
  allNodes: [],
  expandDepth: 2,
  query: "",
};

function buildNode(node, depth, edge, parentUl) {
  const li = el("li", "node-item");
  const row = el("div", "node");

  const childKeys = Object.keys(node.children).sort((a, b) => responseIndex(a) - responseIndex(b));
  const hasChildren = childKeys.length > 0;

  const toggle = el("button", "toggle", hasChildren ? "\u25B8" : "\u00b7");
  toggle.disabled = !hasChildren;
  if (!hasChildren) toggle.classList.add("leaf");
  row.appendChild(toggle);

  row.appendChild(edge === null ? el("span", "edge root", "start") : tileRow(edge, "edge"));
  row.appendChild(tileRow(node.guess, "letter"));
  row.appendChild(el("span", "meta", hasChildren ? childKeys.length + " out" : "win"));

  li.appendChild(row);

  const ul = el("ul", "children");
  ul.hidden = true;
  li.appendChild(ul);

  const entry = { node, li, ul, depth, childKeys, rendered: false, guess: node.guess };
  app.allNodes.push(entry);
  li._entry = entry;

  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!entry.rendered) {
      for (const r of entry.childKeys) {
        buildNode(node.children[r], depth + 1, r, ul);
      }
      entry.rendered = true;
    }
    ul.hidden = !ul.hidden;
    toggle.textContent = ul.hidden ? "\u25B8" : "\u25BE";
  });

  parentUl.appendChild(li);
  if (hasChildren && depth + 1 < app.expandDepth) {
    toggle.click();
  }
  return li;
}

function render() {
  const main = document.getElementById("tree");
  main.textContent = "";
  app.allNodes = [];
  if (!app.file) return;
  const rootUl = el("ul", "children root-children");
  buildNode(app.file.root, 0, null, rootUl);
  main.appendChild(rootUl);
  applySearch();
}

function expandToDepth(d) {
  app.expandDepth = d;
  render();
}

function setAll(expand) {
  // Render everything first (idempotent), then show/hide.
  const entries = [...app.allNodes];
  for (const entry of entries) {
    if (expand && !entry.rendered && entry.childKeys.length) {
      const toggle = entry.li.querySelector(".toggle");
      toggle.click();
    }
  }
  for (const entry of app.allNodes) {
    if (!entry.childKeys.length) continue;
    const toggle = entry.li.querySelector(".toggle");
    if (expand && entry.ul.hidden) toggle.click();
    if (!expand && !entry.ul.hidden) toggle.click();
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
  const searchEl = document.getElementById("search");
  if (q.length > 0) {
    searchEl.classList.toggle("no-match", matches === 0);
  } else {
    searchEl.classList.remove("no-match");
  }
}

function showStats(file, report) {
  const box = document.getElementById("stats");
  box.hidden = false;
  const s = report.stats;
  box.textContent = "";
  const items = [
    ["strategy", file.strategy],
    ["candidates", s.candidates.toLocaleString()],
    ["nodes", s.nodes.toLocaleString()],
    ["edges", s.edges.toLocaleString()],
    ["max depth", s.maxDepth],
    ["mean guesses", s.meanGuesses.toFixed(4)],
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
  badge.appendChild(
    el("span", "v", report.ok ? "\u2713 valid (edge iff possible)" : "\u2717 " + report.errors.length + " problem(s)")
  );
  box.appendChild(badge);

  const status = document.getElementById("status");
  if (report.ok) {
    status.className = "status ok";
    status.textContent = `Validated ${s.nodes.toLocaleString()} nodes / ${s.candidates.toLocaleString()} candidates \u2014 every edge corresponds to a possible response.`;
  } else {
    status.className = "status bad";
    status.innerHTML = "INVALID:<br>" + report.errors.slice(0, 12).map(escapeHtml).join("<br>");
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
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
  status.className = "status";
  status.textContent = "Validating\u2026";
  app.file = file;
  // Yield so the "Validating…" text paints before a possibly-large walk.
  setTimeout(() => {
    const report = validateTree(file);
    app.report = report;
    document.getElementById("depth").max = String(Math.max(1, report.stats.maxDepth));
    showStats(file, report);
    render();
  }, 0);
}

function loadExample(name) {
  const status = document.getElementById("status");
  status.className = "status";
  status.textContent = `Loading ${name}\u2026`;
  fetch(`examples/${name}.json`)
    .then((r) => {
      if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
      return r.json();
    })
    .then(loadFile)
    .catch((e) => {
      status.className = "status bad";
      status.textContent = `Could not load examples/${name}.json: ${e.message}`;
    });
}

// --- wiring ---
document.querySelectorAll("button.example").forEach((b) =>
  b.addEventListener("click", () => loadExample(b.dataset.example))
);

document.getElementById("file").addEventListener("change", (e) => {
  const f = e.target.files[0];
  if (!f) return;
  const reader = new FileReader();
  reader.onload = () => {
    try {
      loadFile(JSON.parse(reader.result));
    } catch (err) {
      const status = document.getElementById("status");
      status.className = "status bad";
      status.textContent = "Could not parse JSON: " + err.message;
    }
  };
  reader.readAsText(f);
});

document.getElementById("expand-all").addEventListener("click", () => setAll(true));
document.getElementById("collapse-all").addEventListener("click", () => setAll(false));

const depthEl = document.getElementById("depth");
depthEl.addEventListener("input", () => {
  document.getElementById("depth-val").textContent = depthEl.value;
  expandToDepth(Number(depthEl.value) + 1);
});

document.getElementById("search").addEventListener("input", (e) => {
  app.query = e.target.value;
  applySearch();
});

// Default view: load the optimal example.
loadExample("optimal");
