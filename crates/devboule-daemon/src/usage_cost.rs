//! The cost number a provider frame carried, sanitized into the wire's
//! optional USD figure.

/// The frame's cost as the wire carries it: finite and non-negative, else
/// `None`. A non-finite value would fail the whole event's serialization,
/// and a negative one bills no turn.
pub(crate) fn finite_cost(cost: f64) -> Option<f64> {
    (cost.is_finite() && cost >= 0.0).then_some(cost)
}

#[cfg(test)]
mod tests {
    use super::finite_cost;

    #[test]
    fn a_cost_the_wire_cannot_carry_is_dropped() {
        assert_eq!(finite_cost(f64::NAN), None);
        assert_eq!(finite_cost(f64::INFINITY), None);
        assert_eq!(finite_cost(f64::NEG_INFINITY), None);
        assert_eq!(finite_cost(-0.5), None);
        // A said-zero stays: the provider billed nothing, its own words.
        assert_eq!(finite_cost(0.0), Some(0.0));
        assert_eq!(finite_cost(0.093081), Some(0.093081));
    }
}
