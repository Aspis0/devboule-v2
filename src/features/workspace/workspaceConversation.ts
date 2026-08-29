import { useCallback, useEffect, useRef, useState } from "react";
import { MOCK_AGENT_REPLY, MOCK_MESSAGES, type MockWorkspaceMessage } from "./mockData";

function cloneMessages(): MockWorkspaceMessage[] {
  return MOCK_MESSAGES.map((message) => ({ ...message }));
}

export function useWorkspaceConversation(onTurnStart: () => void) {
  const [messages, setMessages] = useState<MockWorkspaceMessage[]>(cloneMessages);
  const [streaming, setStreaming] = useState(false);

  const streamTimerRef = useRef<number | null>(null);
  const streamIntervalRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (streamTimerRef.current !== null) window.clearTimeout(streamTimerRef.current);
      if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
    };
  }, []);

  const handleSend = useCallback(
    (messageText: string) => {
      const text = messageText.trim();
      if (!text) return;

      if (streamTimerRef.current !== null) window.clearTimeout(streamTimerRef.current);
      if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
      streamTimerRef.current = null;
      streamIntervalRef.current = null;

      setMessages((currentMessages) => [
        ...currentMessages,
        { id: Date.now(), role: "user", text },
      ]);
      onTurnStart();
      setStreaming(false);

      streamTimerRef.current = window.setTimeout(() => {
        const agentId = Date.now();
        let index = 0;
        setStreaming(true);
        setMessages((currentMessages) => [
          ...currentMessages,
          { id: agentId, role: "agent", text: "" },
        ]);

        streamIntervalRef.current = window.setInterval(() => {
          index += 3;
          const nextText = MOCK_AGENT_REPLY.slice(0, index);
          setMessages((currentMessages) =>
            currentMessages.map((message) =>
              message.id === agentId ? { ...message, text: nextText } : message,
            ),
          );

          if (index >= MOCK_AGENT_REPLY.length) {
            if (streamIntervalRef.current !== null) window.clearInterval(streamIntervalRef.current);
            streamIntervalRef.current = null;
            setStreaming(false);
          }
        }, 24);
      }, 240);
    },
    [onTurnStart],
  );

  return { messages, streaming, handleSend };
}
