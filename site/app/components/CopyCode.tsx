"use client";

import { useEffect } from "react";

// Copy buttons are server-rendered hidden so they only appear once this
// delegated handler is running.
export function CopyCode() {
  useEffect(() => {
    for (const button of document.querySelectorAll<HTMLButtonElement>("button[data-copy-code]")) button.hidden = false;

    async function copy(event: MouseEvent) {
      const button = (event.target as Element | null)?.closest<HTMLButtonElement>("button[data-copy-code]");
      const code = button?.closest(".code-block")?.querySelector("code");
      if (!button || !code) return;
      const text = code.textContent?.replace(/\n$/, "") ?? "";
      let label = "Copied";
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        // Embedded or permission-restricted contexts can reject the async API.
        if (!copyWithSelection(text)) label = "Copy failed";
        button.focus({ preventScroll: true });
      }
      button.textContent = label;
      window.setTimeout(() => { button.textContent = "Copy"; }, 1600);
    }

    document.addEventListener("click", copy);
    return () => document.removeEventListener("click", copy);
  }, []);

  return null;
}

function copyWithSelection(text: string) {
  const field = document.createElement("textarea");
  field.value = text;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.append(field);
  field.select();
  try {
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    field.remove();
  }
}
