// Task board web layer.
//
// The Rust side owns all state. This file does three things:
//   1. pull a render snapshot about every 200 ms (`taskboard_snapshot`),
//   2. draw the session groups and cards, and delete a card on click,
//   3. report drag and hover input so Rust can move the window and park it
//      above the screen top edge.
//
// The window is not resized. When it is parked, only its bottom band stays
// visible at the screen top edge. This file switches between the full shell and
// that band.

"use strict";

const POLL_MS = 200;
const STATE_EVENT = "taskboard://window-state";

const tauri = window.__TAURI__ ?? null;
const invoke = tauri?.core?.invoke ?? null;
const listen = tauri?.event?.listen ?? null;

const el = {
  body: document.body,
  list: document.getElementById("list"),
  summary: document.getElementById("summary"),
  statusLeft: document.getElementById("status-left"),
  statusRight: document.getElementById("status-right"),
  handle: document.getElementById("handle"),
  statusLight: document.getElementById("status-light"),
  dragBar: document.getElementById("drag-bar"),
  closeButton: document.getElementById("close-button"),
  parkButton: document.getElementById("park-button"),
};

let lastRevision = -1;
let parked = false;
let refreshing = false;
let drag = null;
let pendingMove = null;
let moveFrame = 0;

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function renderList(snapshot) {
  const fragment = document.createDocumentFragment();

  if (!snapshot.groups || snapshot.groups.length === 0) {
    fragment.append(emptyState());
  } else {
    fragment.append(tableHead());
    for (const group of snapshot.groups) {
      fragment.append(renderGroup(group));
    }
  }

  el.list.replaceChildren(fragment);
}

function emptyState() {
  const box = document.createElement("div");
  box.className = "empty";
  const title = document.createElement("p");
  title.className = "empty-title";
  title.textContent = "No finished tasks yet";
  const hint = document.createElement("p");
  hint.className = "empty-hint";
  hint.textContent = "Each agent_settled event adds one card here.";
  box.append(title, hint);
  return box;
}

function tableHead() {
  const head = document.createElement("div");
  head.className = "thead";
  for (const label of ["Task", "Start / req", "Took", ""]) {
    const cell = document.createElement("span");
    cell.textContent = label;
    head.append(cell);
  }
  return head;
}

function renderGroup(group) {
  const section = document.createElement("section");
  section.className = "group";
  section.dataset.sessionKey = group.session_key;

  const head = document.createElement("div");
  head.className = "group-head";

  const name = document.createElement("span");
  name.className = "group-name";
  name.textContent = group.heading;
  name.title = `${group.heading} (from ${group.heading_source})`;
  head.append(name);

  if (group.is_ungrouped) {
    head.append(chip("ungrouped"));
  }
  if (typeof group.version === "number") {
    head.append(chip(`v${group.version}`));
  }
  if (group.persisted === true) {
    head.append(chip("persisted"));
  }

  const count = document.createElement("span");
  count.className = "group-count";
  count.textContent = String(group.card_count);
  count.title = `${group.card_count} card(s) in this session`;
  head.append(count);

  section.append(head);
  for (const card of group.cards) {
    section.append(renderCard(card));
  }
  return section;
}

function chip(text) {
  const span = document.createElement("span");
  span.className = "chip";
  span.textContent = text;
  return span;
}

function renderCard(card) {
  const row = document.createElement("div");
  row.className = "row";
  row.dataset.cardId = String(card.id);
  row.dataset.titleSource = card.title_source;
  row.dataset.startSource = card.start_source ?? "none";
  row.tabIndex = 0;
  row.setAttribute(
    "aria-label",
    `${card.title}. Press Enter for the full details.`,
  );

  const titleCell = document.createElement("div");
  titleCell.className = "cell-title";

  const main = document.createElement("span");
  main.className = "main";
  main.textContent = card.title;

  const sub = document.createElement("span");
  sub.className = "sub";
  if (card.secondary) {
    sub.textContent = card.secondary.value;
  } else {
    sub.textContent = "\u00a0";
  }
  titleCell.append(main, sub);

  const times = document.createElement("div");
  times.className = "times";

  const start = document.createElement("span");
  start.className = "start";
  start.textContent = card.start_clock ?? "—";

  const request = document.createElement("span");
  request.className = "request";
  request.textContent = card.request_clock ?? "—";

  times.append(start, request);

  const took = document.createElement("div");
  took.className = card.duration_anomaly ? "took anomaly" : "took";
  took.textContent = card.duration_label;

  const del = document.createElement("button");
  del.className = "del";
  del.type = "button";
  del.textContent = "×";
  del.title = "Delete this card";
  del.setAttribute("aria-label", `Delete card ${card.id}`);
  del.addEventListener("click", () => deleteCard(card.id));
  row.append(titleCell, times, took, del);

  row.addEventListener("pointerenter", () => previewDetail(row));
  row.addEventListener("pointerleave", () => leaveDetail(row));
  row.addEventListener("click", (event) => {
    if (event.target.closest("button")) {
      return;
    }
    toggleDetail(row);
  });
  row.addEventListener("keydown", (event) => {
    if (event.target.closest("button")) {
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      toggleDetail(row);
    }
  });
  return row;
}

function renderStatus(snapshot) {
  const cards = snapshot.card_count ?? 0;
  const sessions = snapshot.session_count ?? 0;

  el.summary.textContent =
    cards === 0
      ? "Waiting for agent_settled events"
      : `${cards} card${cards === 1 ? "" : "s"} in ${sessions} session${sessions === 1 ? "" : "s"}`;

  el.statusLeft.textContent = `${snapshot.zone_label}${
    snapshot.zone_fallback ? " (fallback)" : ""
  } · local display`;
  el.statusRight.textContent = snapshot.generated_at_local ?? "";
}

// ---------------------------------------------------------------------------
// Snapshot loop
// ---------------------------------------------------------------------------

function applySnapshot(snapshot) {
  const scale = snapshot.window?.scale_factor || 1;
  const band = snapshot.window?.parked_band_px ?? 8;
  document.documentElement.style.setProperty(
    "--band-height",
    `${band / scale}px`,
  );

  setParked(Boolean(snapshot.window?.parked));
  setBlinking(Boolean(snapshot.window?.unseen_cards));

  if (snapshot.revision !== lastRevision) {
    lastRevision = snapshot.revision;
    hideDetail();
    renderList(snapshot);
  }

  renderStatus(snapshot);
}

async function refresh() {
  if (!invoke || refreshing) {
    return;
  }
  refreshing = true;
  try {
    const snapshot = await invoke("taskboard_snapshot");
    applySnapshot(snapshot);
  } catch (error) {
    logError("snapshot", error);
  } finally {
    refreshing = false;
  }
}

// ---------------------------------------------------------------------------
// Parked band
// ---------------------------------------------------------------------------

// Length of the Rust slide plus a little slack. The list stays on the screen
// while the window slides up, and the band appears when the slide is done.
const SLIDE_MS = 180;

let parkTimer = 0;

function setParked(next) {
  if (next === parked) {
    return;
  }
  parked = next;
  window.clearTimeout(parkTimer);

  if (parked) {
    cancelDrag();
    hideDetail();
    parkTimer = window.setTimeout(() => {
      el.body.classList.add("is-parked");
      el.handle.hidden = false;
    }, SLIDE_MS);
    return;
  }

  el.body.classList.remove("is-parked");
  el.handle.hidden = true;
  setBlinking(false);
}

// Status light of the band. It blinks until the user opens the window.
//
// The lamp is switched in JavaScript, not by a CSS animation. This keeps the
// blink when the system asks for reduced motion, and it also works when the
// engine does not run CSS animations for a window at the screen edge.
const BLINK_MS = 450;

let blinking = false;
let blinkTimer = 0;

function setBlinking(next) {
  if (next === blinking) {
    return;
  }
  blinking = next;
  el.body.classList.toggle("is-blinking", blinking);
  if (blinking) {
    startBlink();
  } else {
    stopBlink();
  }
}

function startBlink() {
  window.clearInterval(blinkTimer);
  setLamp("on");
  blinkTimer = window.setInterval(() => {
    setLamp(el.statusLight.dataset.state === "on" ? "off" : "on");
  }, BLINK_MS);
}

function stopBlink() {
  window.clearInterval(blinkTimer);
  blinkTimer = 0;
  setLamp("idle");
}

function setLamp(state) {
  el.statusLight.dataset.state = state;
  el.statusLight.setAttribute(
    "aria-label",
    state === "idle" ? "No new tasks" : "New tasks arrived",
  );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

async function deleteCard(cardId) {
  if (!invoke) {
    return;
  }
  hideDetail();
  try {
    const removed = await invoke("taskboard_delete_card", { cardId });
    if (removed) {
      const row = el.list.querySelector(`.row[data-card-id="${cardId}"]`);
      if (row) {
        row.remove();
      }
    }
    await refresh();
  } catch (error) {
    logError("delete", error);
  }
}

// ---------------------------------------------------------------------------
// Card detail
// ---------------------------------------------------------------------------

// A hover opens the panel after this delay. A click skips the delay.
const DETAIL_DELAY_MS = 280;

let detailRow = null;
let detailPinned = false;
let detailTimer = 0;

function previewDetail(row) {
  if (detailPinned) {
    return;
  }
  window.clearTimeout(detailTimer);
  detailTimer = window.setTimeout(() => openDetail(row, false), DETAIL_DELAY_MS);
}

function leaveDetail(row) {
  if (detailPinned) {
    return;
  }
  window.clearTimeout(detailTimer);
  detailTimer = 0;
  if (detailRow === row) {
    hideDetail();
  }
}

function toggleDetail(row) {
  window.clearTimeout(detailTimer);
  detailTimer = 0;
  if (detailPinned && detailRow === row) {
    hideDetail();
    return;
  }
  openDetail(row, true);
}

// Ask Rust to show the panel in the detail window. The panel sits next to this
// window, at the height of the row in CSS px.
function openDetail(row, pinned) {
  const cardId = Number(row.dataset.cardId);
  if (!invoke || !Number.isFinite(cardId)) {
    return;
  }
  detailRow = row;
  detailPinned = pinned;
  markDetailedRow(row);
  const rect = row.getBoundingClientRect();
  invokeSafe("taskboard_show_detail", { cardId, rowTopCss: rect.top });
}

function markDetailedRow(row) {
  for (const other of el.list.querySelectorAll(".row.is-detailed")) {
    if (other !== row) {
      other.classList.remove("is-detailed");
    }
  }
  row.classList.add("is-detailed");
}

function hideDetail() {
  window.clearTimeout(detailTimer);
  detailTimer = 0;
  const wasOpen = detailRow !== null;
  detailRow = null;
  detailPinned = false;
  for (const row of el.list.querySelectorAll(".row.is-detailed")) {
    row.classList.remove("is-detailed");
  }
  if (wasOpen) {
    invokeSafe("taskboard_hide_detail");
  }
}

// A click outside the rows closes a pinned panel.
document.addEventListener("pointerdown", (event) => {
  if (!detailPinned) {
    return;
  }
  if (event.target.closest(".row")) {
    return;
  }
  hideDetail();
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    hideDetail();
  }
});

el.list.addEventListener("scroll", hideDetail);

// ---------------------------------------------------------------------------
// Drag
// ---------------------------------------------------------------------------

el.dragBar.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || event.target.closest("button")) {
    return;
  }
  event.preventDefault();
  hideDetail();

  drag = { id: event.pointerId, x: event.screenX, y: event.screenY };
  el.body.classList.add("dragging");
  try {
    el.dragBar.setPointerCapture(event.pointerId);
  } catch (error) {
    logError("capture", error);
  }
  invokeSafe("taskboard_drag_begin");
});

el.dragBar.addEventListener("pointermove", (event) => {
  if (!drag || event.pointerId !== drag.id) {
    return;
  }
  pendingMove = { dx: event.screenX - drag.x, dy: event.screenY - drag.y };
  if (!moveFrame) {
    moveFrame = window.requestAnimationFrame(flushMove);
  }
});

function flushMove() {
  moveFrame = 0;
  const move = pendingMove;
  pendingMove = null;
  if (!drag || !move) {
    return;
  }
  invokeSafe("taskboard_drag_move", { dx: move.dx, dy: move.dy });
}

function endDrag() {
  const pointerId = drag ? drag.id : null;
  drag = null;
  pendingMove = null;
  el.body.classList.remove("dragging");
  if (pointerId !== null) {
    releasePointer(pointerId);
  }
  invokeSafe("taskboard_drag_end");
  refresh();
}

function cancelDrag() {
  if (!drag) {
    return;
  }
  const pointerId = drag.id;
  drag = null;
  pendingMove = null;
  el.body.classList.remove("dragging");
  releasePointer(pointerId);
  invokeSafe("taskboard_drag_end");
}

function releasePointer(pointerId) {
  try {
    if (el.dragBar.hasPointerCapture(pointerId)) {
      el.dragBar.releasePointerCapture(pointerId);
    }
  } catch (error) {
    logError("release", error);
  }
}

el.dragBar.addEventListener("pointerup", endDrag);
el.dragBar.addEventListener("pointercancel", endDrag);
window.addEventListener("blur", endDrag);

// ---------------------------------------------------------------------------
// Close and park buttons
// ---------------------------------------------------------------------------

el.closeButton.addEventListener("click", () => {
  invokeSafe("taskboard_quit");
});

el.parkButton.addEventListener("click", () => {
  invokeSafe("taskboard_park");
});

el.handle.addEventListener("pointerenter", () => {
  invokeSafe("taskboard_hover", { inside: true });
});

el.handle.addEventListener("pointerleave", () => {
  invokeSafe("taskboard_hover", { inside: false });
});

el.handle.addEventListener("pointerdown", (event) => {
  event.preventDefault();
  invokeSafe("taskboard_expand");
});

el.handle.addEventListener("keydown", (event) => {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    invokeSafe("taskboard_expand");
  }
});

// ---------------------------------------------------------------------------
// Helpers and start
// ---------------------------------------------------------------------------

function invokeSafe(command, args) {
  if (!invoke) {
    return Promise.resolve();
  }
  return invoke(command, args).catch((error) => logError(command, error));
}

function logError(where, error) {
  console.error(`[taskboard] ${where}:`, error);
}

function listenState() {
  if (!listen) {
    return;
  }
  listen(STATE_EVENT, (event) => {
    const payload = event?.payload ?? {};
    setParked(Boolean(payload.parked));
    refresh();
  }).catch((error) => logError("listen", error));
}

function start() {
  if (!invoke) {
    el.summary.textContent = "Tauri IPC is not available";
    return;
  }
  listenState();
  refresh();
  window.setInterval(refresh, POLL_MS);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start);
} else {
  start();
}
