/**
 * The outcome a permission option posts back to the daemon.
 *
 * The wire rule for an agent's own options: an option whose kind is `allow*`
 * posts the choice (`allow_once`); everything else — a `reject*` kind and a
 * kind this mapping does not recognize alike — posts the refusal (`deny`).
 * The default is deliberate and closed: a kind it does not know must never be
 * sent, or styled, as a grant. The broker validates the pairing
 * (`select_option`) either way, so an unknown kind is refused there rather
 * than answered here.
 */
export function optionOutcome(kind: string): "allow_once" | "deny" {
  return kind.startsWith("allow") ? "allow_once" : "deny";
}
