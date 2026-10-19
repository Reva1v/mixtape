// One Esc closes one layer. Open overlays (full-screen player, modals, the context menu)
// register here while open; Esc goes only to the most recently opened one. The listener
// runs in the capture phase on window and stops the event, so sibling window listeners
// (other layers, global hotkeys) never see an Esc that already closed something.

type Close = () => void;

const stack: { close: Close }[] = [];

function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.isComposing || !stack.length) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  stack[stack.length - 1].close();
}

if (typeof window !== "undefined") window.addEventListener("keydown", onKey, true);

/** Puts a layer on top of the Esc stack; the returned function removes it. Fits `$effect` cleanup. */
export function escapeLayer(close: Close): () => void {
  const entry = { close };
  stack.push(entry);
  return () => {
    const i = stack.indexOf(entry);
    if (i >= 0) stack.splice(i, 1);
  };
}
