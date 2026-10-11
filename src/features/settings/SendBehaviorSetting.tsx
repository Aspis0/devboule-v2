import { useId, useSyncExternalStore } from "react";
import { ChoiceGroup } from "../../components/ChoiceGroup";
import { commandKeyPrefix } from "../../lib/keymap";
import {
  getSendBehavior,
  setSendBehavior,
  subscribeSendBehavior,
  type SendBehavior,
} from "../../lib/sendBehavior";
import { SettingsRow } from "./rows";

interface SendOption {
  value: SendBehavior;
  label: string;
  /** The one line the row states for this option; `command` is "Ctrl+" or "⌘". */
  line: (command: string) => string;
}

/** The three choices and their default follow Paseo (storage.ts:219). */
const OPTIONS: readonly SendOption[] = [
  {
    value: "steer-and-send",
    label: "Steer",
    line: (command) => `Enter adds your message to the running turn · ${command}Enter queues`,
  },
  {
    value: "interrupt-and-send",
    label: "Interrupt",
    line: (command) => `Enter stops the turn and sends · ${command}Enter queues`,
  },
  {
    value: "queue",
    label: "Queue",
    line: (command) => `Enter waits for the turn to end · ${command}Enter stops and sends`,
  },
];

/** The value lives in the sendBehavior store, so a change re-renders every mounted surface that reads it. */
export function SendBehaviorSetting() {
  const behavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);
  const descriptionId = useId();
  const selected = OPTIONS.find((option) => option.value === behavior);

  return (
    <SettingsRow
      title="Enter while the agent runs"
      description={selected?.line(commandKeyPrefix())}
      descriptionId={descriptionId}
      control={
        <ChoiceGroup
          options={OPTIONS}
          value={behavior}
          onChange={setSendBehavior}
          ariaLabel="Enter while the agent runs"
          describedBy={descriptionId}
        />
      }
    />
  );
}
