/**
 * The outcome a permission option posts back to the daemon.
 *
 * The wire rule for an agent's own options: an option whose kind is `allow*`
 * posts the choice (`allow_once`), one whose kind is `reject*` posts the
 * refusal (`deny`). A kind that is neither is still sent as the choice — the
 * broker validates the pairing (`select_option`) and refuses what it cannot
 * honor, because a wrong grant would be worse than a refusal the daemon
 * reports.
 */
export function optionOutcome(kind: string): "allow_once" | "deny" {
  return kind.startsWith("reject") ? "deny" : "allow_once";
}
