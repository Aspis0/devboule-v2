import { useSyncExternalStore } from "react";
import {
  getSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "../../lib/sendBehavior";
import "./settings.css";

/** Paseo's "Default send" choice, reduced to this app's two behaviours and put
 * in the row form every other General-tab setting uses. The copy says what the
 * alternate key actually does: under the queue default it is not a second
 * submit — it interrupts the running turn and sends (decision 3), the same act
 * the composer's "Send and interrupt" button names. The value lives in the
 * sendBehavior store, so a change here re-renders every mounted surface that
 * reads it. */

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
    <div className="settings-card settings-value-row send-behavior">
      <span className="settings-card-copy">
        <span className="settings-card-title">Default send</span>
        <span className="send-behavior-options" role="radiogroup" aria-label="Default send">
          {OPTIONS.map((option) => (
            <label className="send-behavior-option" key={option.value}>
              <input
                type="radio"
                name="send-behavior"
                value={option.value}
                checked={behavior === option.value}
                onChange={() => setSendBehavior(option.value)}
              />
              <span>
                <span>{option.label}</span>
                <span className="send-behavior-note">{option.description}</span>
              </span>
            </label>
          ))}
        </span>
      </span>
    </div>
  );
}
