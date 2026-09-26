import { AppearanceSection } from "../AppearanceSection";
import { CloseBehaviorSetting } from "../CloseBehaviorSetting";
import { SendBehaviorSetting } from "../SendBehaviorSetting";
import { JournalRetentionPanel } from "../JournalRetentionPanel";
export function GeneralPanel() {
  return (
    <div id="settings-panel-general" role="tabpanel" aria-label="General">
      <AppearanceSection />
      <CloseBehaviorSetting />
      <SendBehaviorSetting />
      <JournalRetentionPanel />
    </div>
  );
}
