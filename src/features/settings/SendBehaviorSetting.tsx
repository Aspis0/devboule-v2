import { useSyncExternalStore } from "react";
import {
  getSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "../../lib/sendBehavior";
import "./general.css";

/** This app's two send behaviours, offered in the row form every other
 * Editing-page setting uses. The copy says what the
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
    <div className="machine-card" aria-labelledby="send-behavior-heading">
      <h3 className="settings-subheading" id="send-behavior-heading">
        Default send
      </h3>
      <div className="machine-choices" role="radiogroup" aria-label="Default send">
        {OPTIONS.map((option) => (
          <label className="machine-choice" key={option.value}>
            <input
              type="radio"
              name="send-behavior"
              value={option.value}
              checked={behavior === option.value}
              onChange={() => setSendBehavior(option.value)}
            />
            <span className="machine-row-copy">
              <span className="machine-row-title">{option.label}</span>
              <span className="machine-row-desc">{option.description}</span>
            </span>
          </label>
        ))}
      </div>
    </div>
  );
}
