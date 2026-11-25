"use client";

import { useEffect } from "react";

// Moves the landing dot-grid spotlight with the pointer. The stage renders
// complete without it.
export function StageSpotlight() {
  useEffect(() => {
    const stage = document.querySelector<HTMLElement>(".home-stage");
    if (!stage || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let frame = 0;
    const move = (event: PointerEvent) => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const rect = stage.getBoundingClientRect();
        stage.style.setProperty("--mx", `${((event.clientX - rect.left) / rect.width) * 100}%`);
        stage.style.setProperty("--my", `${((event.clientY - rect.top) / rect.height) * 100}%`);
      });
    };
    stage.addEventListener("pointermove", move);
    return () => { cancelAnimationFrame(frame); stage.removeEventListener("pointermove", move); };
  }, []);
  return null;
}
