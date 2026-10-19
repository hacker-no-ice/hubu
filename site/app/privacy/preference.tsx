"use client";

import { useState, useSyncExternalStore } from "react";

function subscribe(listener: () => void) {
  window.addEventListener("focus", listener);
  window.addEventListener("hubu-analytics-preference", listener);
  return () => {
    window.removeEventListener("focus", listener);
    window.removeEventListener("hubu-analytics-preference", listener);
  };
}

function snapshot() {
  const excluded = document.cookie.split(";").some((part) => part.trim() === "hubu_analytics_exclude=1");
  const signal = navigator.doNotTrack === "1" || (navigator as Navigator & { globalPrivacyControl?: boolean }).globalPrivacyControl === true;
  return `${excluded}:${signal}`;
}

export function AnalyticsPreference() {
  const preference = useSyncExternalStore(subscribe, snapshot, () => "");
  const excluded = preference === "" ? null : preference.startsWith("true:");
  const privacySignal = preference.endsWith(":true");
  const [message, setMessage] = useState("");

  function toggle() {
    const next = !excluded;
    document.cookie = `hubu_analytics_exclude=${next ? "1" : ""}; Path=/; SameSite=Lax; Secure; Max-Age=${next ? 31536000 : 0}`;
    const saved = document.cookie.split(";").some((part) => part.trim() === "hubu_analytics_exclude=1");
    window.dispatchEvent(new Event("hubu-analytics-preference"));
    setMessage(saved === next ? (next ? "This browser is now excluded." : "The exclusion preference has been removed.") : "Your browser did not save this preference. You can also enable Do Not Track or Global Privacy Control.");
  }

  return (
    <section aria-label="Analytics preference">
      <p role="status">{message || (excluded === null ? "Checking this browser’s preference…" : privacySignal ? "Your browser’s privacy signal already disables tracking." : excluded ? "This browser is excluded." : "This browser has not opted out.")}</p>
      <button className="button secondary" type="button" disabled={excluded === null} onClick={toggle}>
        {excluded ? "Remove browser exclusion" : "Exclude this browser"}
      </button>
    </section>
  );
}
