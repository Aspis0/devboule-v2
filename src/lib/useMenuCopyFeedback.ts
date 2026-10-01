import { useEffect, useRef, useState } from "react";
import { copyToClipboard } from "./clipboard";

export function useMenuCopyFeedback() {
  const [feedback, setFeedback] = useState<{
    key: string;
    label: string;
    announcement: string;
  } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const request = useRef(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      request.current += 1;
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);

  async function copy(key: string, value: string, subject: string) {
    const current = ++request.current;
    if (timer.current !== null) clearTimeout(timer.current);
    setFeedback(null);
    const copied = await copyToClipboard(value);
    // A later copy or an unmounted menu must not receive an old completion.
    if (!mounted.current || current !== request.current) return;
    setFeedback({
      key,
      label: copied ? "Copied" : "Copy failed",
      announcement: `${subject} ${copied ? "copied" : "copy failed"}`,
    });
    timer.current = setTimeout(() => setFeedback(null), 1500);
  }

  return {
    copy,
    labelFor: (key: string, label: string) => (feedback?.key === key ? feedback.label : label),
    announcement: feedback?.announcement ?? "",
  };
}
