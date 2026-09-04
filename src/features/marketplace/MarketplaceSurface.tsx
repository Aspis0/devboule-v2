import "./marketplace.css";

import { useMemo, useState } from "react";
import { pluginState } from "../../lib/plugins";
import { useAppStore } from "../../store/appStore";
import { MOCK_MARKETPLACE_ENTRIES, type MarketplaceEntry, type MarketplaceKind } from "./mockData";

type MarketplaceFilter = "all" | MarketplaceKind;

const FILTERS: readonly { id: MarketplaceFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "plugin", label: "Plugins" },
  { id: "pack", label: "Packs" },
  { id: "skill", label: "Skills" },
];

export function skillIsInstallable(entry: MarketplaceEntry): boolean {
  return entry.kind === "skill" && entry.price === "free";
}

export function MarketplaceSurface() {
  const [filter, setFilter] = useState<MarketplaceFilter>("all");
  const [notice, setNotice] = useState<string | null>(null);
  const installedSkills = useAppStore((state) => state.installedSkills);
  const installSkill = useAppStore((state) => state.installSkill);
  const plugins = useAppStore((state) => state.plugins);
  const installedSkillIds = useMemo(
    () => new Set(installedSkills.map((skill) => skill.id)),
    [installedSkills],
  );
  const entries = MOCK_MARKETPLACE_ENTRIES.filter(
    (entry) => filter === "all" || entry.kind === filter,
  );

  function handleBuy() {
    setNotice("Purchases are not wired yet.");
  }

  function handleSkillInstall(entry: MarketplaceEntry) {
    if (!skillIsInstallable(entry)) return;
    installSkill({
      id: entry.id,
      name: entry.name,
      author: entry.author,
      description: entry.description,
    });
    setNotice(
      "Installed in Workspace for this session only — the skill list is not saved to disk.",
    );
  }

  return (
    <section className="surface-card marketplace-surface" aria-labelledby="marketplace-title">
      <header className="marketplace-header">
        <div className="marketplace-header-title">
          <h1 id="marketplace-title">Marketplace</h1>
          <span className="marketplace-header-divider" aria-hidden="true" />
          <span className="marketplace-eyebrow">plugins · packs · skills</span>
        </div>
        <span className="marketplace-header-note">
          Free skills appear in Workspace; this session&apos;s list is not saved to disk.
        </span>
      </header>

      <p className="marketplace-demo-note">Demo catalog — fixtures, not a live store.</p>

      <div className="marketplace-toolbar">
        <div className="marketplace-filters" role="group" aria-label="Marketplace filters">
          {FILTERS.map((item) => (
            <button
              type="button"
              aria-pressed={filter === item.id}
              className={`marketplace-filter${filter === item.id ? " marketplace-filter-active" : ""}`}
              key={item.id}
              onClick={() => setFilter(item.id)}
            >
              {item.label}
            </button>
          ))}
        </div>
        <span className="marketplace-count">{entries.length} listings</span>
      </div>

      {notice ? (
        <div className="marketplace-notice" role="alert">
          {notice}
        </div>
      ) : null}

      <div className="marketplace-list-wrap">
        {entries.length > 0 ? (
          <ul className="marketplace-list">
            {entries.map((entry) => {
              const installed = installedSkillIds.has(entry.id);
              const installedPlugin =
                entry.kind === "plugin" && plugins !== null ? pluginState(plugins, entry.id) : null;
              return (
                <li
                  className="marketplace-row"
                  data-entry-id={entry.id}
                  data-kind={entry.kind}
                  key={entry.id}
                >
                  <div className="marketplace-row-main">
                    <div className="marketplace-row-title">
                      <h2>{entry.name}</h2>
                      <span className="marketplace-price">{entry.price}</span>
                    </div>
                    <div className="marketplace-row-meta">
                      <span>{entry.author}</span>
                      <span aria-hidden="true">·</span>
                      <span>{entry.kind}</span>
                    </div>
                    <p>{entry.description}</p>
                  </div>

                  <div className="marketplace-row-actions">
                    {skillIsInstallable(entry) ? (
                      <button
                        type="button"
                        className="marketplace-action marketplace-action-install"
                        data-action="install-skill"
                        onClick={() => handleSkillInstall(entry)}
                        disabled={installed}
                      >
                        {installed ? "Installed" : "Install"}
                      </button>
                    ) : entry.kind === "plugin" ? (
                      <span
                        className={`marketplace-plugin-status${installedPlugin?.kind === "ready" ? " marketplace-plugin-status-ready" : ""}`}
                      >
                        {installedPlugin?.kind === "ready"
                          ? "Installed"
                          : installedPlugin?.kind === "unknown"
                            ? `Install status unavailable — ${installedPlugin.problem}`
                            : installedPlugin?.kind === "refused"
                              ? "Installed but unavailable"
                              : plugins === null
                                ? "Checking plugin status."
                                : "Install from the crescent +."}
                      </span>
                    ) : (
                      <button
                        type="button"
                        className="marketplace-action marketplace-action-buy"
                        data-action="buy"
                        aria-label={`Buy ${entry.name}`}
                        onClick={handleBuy}
                      >
                        Buy
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="marketplace-empty" role="alert">
            No listings in this filter.
          </p>
        )}
      </div>
    </section>
  );
}
