import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useTrackedRequest } from "../../../lib/trackedRequest";
import { daemonDiagnostics } from "../../../lib/tauri";
import { SettingsAdvanced, SettingsRow, SettingsSection } from "../rows";
import { useSettingsDaemon } from "../settingsDaemon";

function FactRow({ label, value }: { label: string; value: string }) {
  return <SettingsRow title={label} control={<span>{value}</span>} />;
}

export function AboutPanel() {
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const status = useSettingsDaemon();
  const { run, state: report } = useTrackedRequest(daemonDiagnostics, { status: "loading" }, true);

  const connection = `${status.state}:${status.instanceId ?? ""}`;
  const lastConnection = useRef(connection);

  // The auto-started read covers the connection at mount; any later change —
  // a reconnect, a restart — reads again, so a dead daemon's numbers do not stay.
  useEffect(() => {
    if (lastConnection.current === connection) return;
    lastConnection.current = connection;
    run();
  }, [connection, run]);

  useEffect(() => {
    let cancelled = false;
    void getVersion()
      .then((version) => {
        if (!cancelled) setAppVersion(version);
      })
      // No row rather than a fallback literal: the version already lives
      // in the config files, and a hardcoded copy would drift from them.
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div id="settings-panel-about">
      <SettingsSection label="This app">
        {appVersion !== null ? <FactRow label="Version" value={appVersion} /> : null}
      </SettingsSection>
      <SettingsSection label="Daemon">
        {report.status === "ready" ? (
          <>
            <FactRow label="Version" value={report.value.daemon.version} />
            <FactRow label="Protocol version" value={String(report.value.daemon.protocolVersion)} />
          </>
        ) : report.status === "error" ? (
          <p className="settings-error" role="alert">
            {report.message}
          </p>
        ) : null}
      </SettingsSection>
      <SettingsSection label="License">
        <FactRow label="Devboule's own code" value="Apache-2.0" />
        <SettingsAdvanced>
          <p>Third-party components keep their own licenses, listed in THIRD_PARTY.md.</p>
        </SettingsAdvanced>
      </SettingsSection>
    </div>
  );
}
