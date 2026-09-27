import { daemonDotTone, daemonLabel } from "../workspace/sidebar/SidebarFooter";
import { useSettingsDaemon } from "./settingsDaemon";

// The Devices group host row's live dot. It owns the surface's only daemon
// subscription, so the shell around it never re-renders on the 2 s poll.
// Unknown or disconnected states never render as live: `daemonDotTone` maps
// everything but connected away from green.
export function HostDot() {
  const daemon = useSettingsDaemon();
  const sentence = daemonLabel(daemon);
  return (
    <span
      className={`settings-host-dot settings-host-dot-${daemonDotTone(daemon.state)}`}
      role="img"
      aria-label={sentence}
      title={sentence}
    />
  );
}
