import { useSyncExternalStore } from "react";
import { ChoiceGroup } from "../../components/ChoiceGroup";
import {
  getSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "../../lib/sendBehavior";
import { SettingsAdvanced, SettingsRow } from "./rows";

/** The three choices, Paseo's own (packages/app/src/hooks/use-settings/storage.ts:219)
 * and its default, steer. Their full effect sits under Advanced: the alternate
 * key queues under steer and interrupt, and interrupts under queue. The value
 * lives in the sendBehavior store, so a change here re-renders every mounted
 * surface that reads it. */
const OPTIONS: readonly { value: SendBehavior; label: string; description: string }[] = [
  {
    value: "steer-and-send",
    label: "Steer",
    description:
      "When the agent is running, Enter steers the message into the running turn. Command/Ctrl+Enter queues.",
  },
  {
    value: "interrupt-and-send",
    label: "Interrupt",
    description:
      "When the agent is running, Enter interrupts the turn and sends. Command/Ctrl+Enter queues.",
  },
  {
    value: "queue",
    label: "Queue",
    description:
      "When the agent is running, Enter queues. Command/Ctrl+Enter interrupts the running turn and sends.",
  },
];

export function SendBehaviorSetting() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);

  return (
    <>
      <SettingsRow
        title="Enter while the agent runs"
        control={
          <ChoiceGroup
            options={OPTIONS}
            value={behavior}
            onChange={setSendBehavior}
            ariaLabel="Enter while the agent runs"
          />
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
