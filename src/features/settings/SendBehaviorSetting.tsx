import { useSyncExternalStore } from "react";
import {
  getSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "../../lib/sendBehavior";
import { SettingsAdvanced, SettingsRow } from "./rows";

/** The two send behaviours. Their full effect sits under Advanced: under the
 * queue default the alternate key is not a second submit — it interrupts the
 * running turn and sends, the act the composer's "Send and interrupt" button
 * names. The value lives in the sendBehavior store, so a change here
 * re-renders every mounted surface that reads it. */
const OPTIONS: readonly { value: SendBehavior; label: string; description: string }[] = [
  {
    value: "queue",
    label: "Queue",
    description:
      "When the agent is running, Enter queues. Command/Ctrl+Enter interrupts the running turn and sends.",
  },
  {
    value: "interrupt-and-send",
    label: "Steer",
    description: "When the agent is running, Enter interrupts. Command/Ctrl+Enter queues.",
  },
];

export function SendBehaviorSetting() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);

  return (
    <>
      <SettingsRow
        title="Enter while the agent runs"
        control={
          <div
            className="settings-choices"
            role="radiogroup"
            aria-label="Enter while the agent runs"
          >
            {OPTIONS.map((option) => (
              <label className="settings-choice" key={option.value}>
                <input
                  type="radio"
                  name="send-behavior"
                  value={option.value}
                  checked={behavior === option.value}
                  onChange={() => setSendBehavior(option.value)}
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
        }
      />
      <SettingsAdvanced>
        {OPTIONS.map((option) => (
          <p key={option.value}>{option.description}</p>
        ))}
      </SettingsAdvanced>
    </>
  );
}
