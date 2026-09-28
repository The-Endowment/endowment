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
    <>
      <span className="address">{address}</span>
      <button type="button" onClick={copy} className="copy" aria-label="Copy the $PENIS mint address">
        {copied ? "Copied" : "Copy"}
      </button>
    </>
  );
}
