"use client";

import { useEffect, useRef } from "react";

const MASCOT = "/brand/hubu-mascot-evolved.jpg";
const ALT = "Marty, Hubu's cream-and-jade seal mascot on a cinnabar base";

// Where Marty comments after being dropped.
const SPOTS: [string, string][] = [
  [".t-policy", "these rules look tidy"],
  [".t-budget", "two whole dollars!"],
  [".t-stack", "all green, nice"],
  [".t-register", "hello, image-agent"],
  [".t-hud", "watching the budget 👀"],
  [".t-harness", "agents never see the keys"],
  [".t-portal", "need a seal? I am one"],
  [".boundary", "home sweet control plane"],
  [".hsearch", "looking for something?"],
];

// Marty perches on the hero's boundary card. Visitors can drag Marty anywhere;
// Marty leans toward the cursor, dozes when idle, and reacts to the stack demo.
export function Marty() {
  const perchRef = useRef<HTMLElement>(null);
  const homeRef = useRef<HTMLImageElement>(null);
  const perchSayRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const perch = perchRef.current, home = homeRef.current, perchSay = perchSayRef.current;
    if (!perch || !home || !perchSay) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    let el: HTMLDivElement | null = null, img: HTMLImageElement | null = null, bubble: HTMLSpanElement | null = null;
    let raf = 0, sayTimer = 0, idleTimer = 0, asleep = false;
    let held = false, px = 0, py = 0, x = 0, y = 0, vx = 0, vy = 0, rot = 0, vr = 0, ox = 0, oy = 0, width = 0;
    let lastDir = 0, flips: number[] = [], fastShown = 0, dizzyUntil = 0, dropAt = 0, pendingLand = false;

    const speak = (text: string, ms = 1400) => {
      const target = bubble ?? perchSay;
      target.textContent = text;
      target.classList.add("show");
      window.clearTimeout(sayTimer);
      sayTimer = window.setTimeout(() => target.classList.remove("show"), ms);
    };
    const body = () => el ?? perch;
    const setAnim = (name: string | null) => {
      if (!el) return;
      el.classList.remove("idle", "land", "dizzy");
      void el.offsetWidth;
      if (name) el.classList.add(name);
    };
    const wake = () => {
      if (asleep) { asleep = false; body().classList.remove("sleepy"); speak("oh! hi", 900); }
      window.clearTimeout(idleTimer);
      idleTimer = window.setTimeout(() => { asleep = true; body().classList.add("sleepy"); speak("z z z", 4000); }, 20000);
    };

    function render() {
      if (!el || !img) return;
      el.style.transform = `translate(${x}px, ${y}px)`;
      img.style.rotate = `${rot}deg`;
      if (reduce) return;
      const speed = Math.hypot(vx, vy), stretch = Math.min(0.22, speed * 0.012), angle = Math.atan2(vy, vx);
      const sx = 1 + stretch * Math.abs(Math.cos(angle)) - stretch * 0.5 * Math.abs(Math.sin(angle));
      const sy = 1 + stretch * Math.abs(Math.sin(angle)) - stretch * 0.5 * Math.abs(Math.cos(angle));
      img.style.scale = `${sx} ${sy}`;
    }
    function step() {
      const k = held ? 0.22 : 0.16, damp = held ? 0.72 : 0.74;
      vx = (vx + (px - x) * k) * damp;
      vy = (vy + (py - y) * k) * damp;
      x += vx; y += vy;
      const target = reduce ? 0 : Math.max(-28, Math.min(28, -vx * 1.6));
      vr = (vr + (target - rot) * 0.18) * 0.78; rot += vr;
      render();
      if (held && Math.hypot(vx, vy) > 38 && performance.now() - fastShown > 1800) { fastShown = performance.now(); speak("wheee!", 900); }
      const settled = !held && Math.abs(px - x) + Math.abs(py - y) < 0.5 && Math.abs(vx) + Math.abs(vy) + Math.abs(vr) < 0.05 && Math.abs(rot) < 0.2;
      if (!settled) { raf = requestAnimationFrame(step); return; }
      raf = 0; rot = 0; render();
      if (pendingLand) { pendingLand = false; land(); }
    }
    const kick = () => { if (!raf) raf = requestAnimationFrame(step); };

    function land() {
      if (!el) return;
      if (dropAt < dizzyUntil) { setAnim("dizzy"); window.setTimeout(() => setAnim("idle"), 1800); return; }
      setAnim("land");
      window.setTimeout(() => setAnim("idle"), 560);
      const rect = el.getBoundingClientRect(), cx = rect.left + rect.width / 2, cy = rect.top + rect.height * 0.7;
      const hit = SPOTS.find(([selector]) => {
        const node = document.querySelector(selector);
        if (!node) return false;
        const box = node.getBoundingClientRect();
        return cx > box.left && cx < box.right && cy > box.top && cy < box.bottom;
      });
      speak(hit ? hit[1] : "double-click to send me home", hit ? 1800 : 2200);
    }

    function create() {
      const rect = home!.getBoundingClientRect();
      width = rect.width;
      el = document.createElement("div");
      el.className = "marty-loose";
      el.style.width = `${width}px`;
      el.setAttribute("role", "img");
      el.setAttribute("aria-label", "Marty. Drag to move, double-click to send home.");
      img = document.createElement("img");
      img.src = MASCOT; img.alt = ""; img.draggable = false;
      bubble = document.createElement("span");
      bubble.className = "marty-say";
      el.append(bubble, img);
      document.body.appendChild(el);
      x = px = rect.left; y = py = rect.top; render();
      perch!.classList.add("away");
      if (asleep) el.classList.add("sleepy");
      el.addEventListener("pointerdown", grab);
      el.addEventListener("dblclick", sendHome);
    }
    function grab(event: PointerEvent) {
      if (!el) return;
      event.preventDefault();
      held = true; pendingLand = false;
      el.classList.add("held"); setAnim(null);
      ox = event.clientX - x; oy = event.clientY - y; px = x; py = y; flips = []; lastDir = 0;
      el.setPointerCapture?.(event.pointerId);
      el.addEventListener("pointermove", move);
      el.addEventListener("pointerup", drop);
      el.addEventListener("pointercancel", drop);
      if (performance.now() > dizzyUntil) speak(["!", "hey!", "whoa"][Math.floor(Math.random() * 3)], 700);
      kick();
    }
    function move(event: PointerEvent) {
      if (!held) return;
      const nx = Math.min(window.innerWidth - width * 0.4, Math.max(-width * 0.6, event.clientX - ox));
      const ny = Math.min(window.innerHeight - width * 0.4, Math.max(0, event.clientY - oy));
      const dir = Math.sign(nx - px);
      if (dir && lastDir && dir !== lastDir) {
        const now = performance.now();
        flips = flips.filter((time) => now - time < 900);
        flips.push(now);
        if (flips.length >= 6 && now > dizzyUntil) { dizzyUntil = now + 2500; flips = []; speak("@_@  so dizzy", 1800); }
      }
      if (dir) lastDir = dir;
      px = nx; py = ny; kick();
    }
    function drop() {
      if (!el) return;
      held = false;
      el.classList.remove("held");
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", drop);
      el.removeEventListener("pointercancel", drop);
      dropAt = performance.now(); pendingLand = true;
      kick();
    }
    function sendHome() {
      if (!el) return;
      const rect = home!.getBoundingClientRect();
      speak("bye!", 600);
      cancelAnimationFrame(raf); raf = 0;
      el.style.transition = reduce ? "" : "transform .5s cubic-bezier(.3,1.3,.5,1)";
      x = px = rect.left; y = py = rect.top; vx = vy = 0; rot = 0; render();
      const leaving = el;
      window.setTimeout(() => { leaving.remove(); perch!.classList.remove("away"); }, reduce ? 0 : 520);
      el = null; img = null; bubble = null;
    }

    const onHomeDown = (event: PointerEvent) => { if (!el) create(); grab(event); };
    const onMove = (event: PointerEvent) => {
      wake();
      if (el || reduce) return;
      const rect = home.getBoundingClientRect(), dx = event.clientX - (rect.left + rect.width / 2), dy = event.clientY - (rect.top + rect.height / 2);
      home.style.setProperty("--lean", `${Math.hypot(dx, dy) < 900 ? Math.max(-10, Math.min(10, dx / 40)) : 0}deg`);
    };
    const onSay = (event: Event) => {
      const { text, ms } = (event as CustomEvent<{ text: string; ms?: number }>).detail;
      const box = body().getBoundingClientRect();
      if (!asleep && box.bottom > 0 && box.top < window.innerHeight) speak(text, ms);
    };

    home.addEventListener("pointerdown", onHomeDown);
    window.addEventListener("pointermove", onMove, { passive: true });
    window.addEventListener("scroll", wake, { passive: true });
    window.addEventListener("marty:say", onSay);
    wake();
    return () => {
      home.removeEventListener("pointerdown", onHomeDown);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("scroll", wake);
      window.removeEventListener("marty:say", onSay);
      window.clearTimeout(idleTimer); window.clearTimeout(sayTimer);
      cancelAnimationFrame(raf);
      el?.remove();
    };
  }, []);

  return (
    <figure className="perch" ref={perchRef}>
      <span className="marty-say" ref={perchSayRef} aria-live="polite" />
      {/* eslint-disable-next-line @next/next/no-img-element -- Static artwork is served directly by the site worker. */}
      <img ref={homeRef} src={MASCOT} width="1254" height="975" alt={ALT} draggable={false} decoding="async" />
      <figcaption><a href="/mascot">Meet Marty</a> · drag me</figcaption>
    </figure>
  );
}
