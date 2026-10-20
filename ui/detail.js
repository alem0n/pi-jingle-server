// Card detail panel.
//
// This page runs in a window of its own. Rust places that window next to the
// task board, so the panel can sit outside the task board window. The page
// renders the card, measures the panel and reports the size; Rust then shows
// the window at the computed place.

const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;

const DETAIL_EVENT = "taskboard://detail";

const el = {
  root: document.getElementById("root"),
  panel: document.getElementById("panel"),
  title: document.getElementById("panel-title"),
  session: document.getElementById("panel-session"),
  lines: document.getElementById("panel-lines"),
};

function render(data) {
  el.title.textContent = data.title;
  el.session.textContent = data.session;
  el.lines.replaceChildren();
  for (const line of data.lines) {
    const label = document.createElement("dt");
    label.textContent = line.label;
    const value = document.createElement("dd");
    value.textContent = line.value;
    if (line.warn) {
      value.className = "warn";
    }
    el.lines.append(label, value);
  }
  el.panel.hidden = false;
  // Measure after the engine has laid out the new rows.
  window.requestAnimationFrame(reportSize);
}

// Send the panel size in CSS px. Rust converts it to physical px and places
// the window.
function reportSize() {
  if (!invoke || el.panel.hidden) {
    return;
  }
  const rect = el.root.getBoundingClientRect();
  const width = Math.ceil(rect.width);
  const height = Math.ceil(rect.height);
  invoke("taskboard_detail_size", { width, height }).catch((error) =>
    console.error("[detail] size:", error),
  );
}

async function start() {
  if (!invoke) {
    return;
  }
  if (listen) {
    listen(DETAIL_EVENT, (event) => {
      if (event?.payload) {
        render(event.payload);
      }
    }).catch((error) => console.error("[detail] listen:", error));
  }
  // The page can load after the first card was sent. Ask for it once.
  try {
    const data = await invoke("taskboard_detail_sync");
    if (data) {
      render(data);
    }
  } catch (error) {
    console.error("[detail] sync:", error);
  }
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start);
} else {
  start();
}
