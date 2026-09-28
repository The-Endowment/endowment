"use client";

import { useState } from "react";

export function CopyAddress({ address }: { address: string }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be blocked; the address stays selectable.
    }
  }

  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 10, flexWrap: "wrap", justifyContent: "flex-end" }}>
      <span className="address">{address}</span>
      <button
        type="button"
        onClick={copy}
        className="chip"
        style={{ cursor: "pointer", background: "transparent", minHeight: 32 }}
        aria-label="Copy the $PENIS mint address"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </span>
  );
}
