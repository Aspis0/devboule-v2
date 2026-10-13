//! Dependency resolution over `requires`: closure and order.
//!
//! One responsibility: close a requested slug list over its dependencies
//! and order the result the composer needs — requested first, shallower
//! next, request order within a tier — exactly as the app resolves it.

use std::collections::{HashMap, HashSet};

use super::design_craft::SkillSection;

#[derive(Debug)]
pub(crate) enum ResolveError {
    // The missing slug: named by the conformance tests, ignored by the
    // tolerant runtime, which composes nothing instead.
    #[cfg_attr(not(test), allow(dead_code))]
    UnknownSlug(String),
    Cycle(Vec<String>),
}

struct ClosureInfo {
    depth: usize,
    explicit: bool,
}

fn visit(
    slug: &str,
    depth: usize,
    explicit: bool,
    by_slug: &HashMap<&str, &SkillSection>,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
    closure: &mut HashMap<String, ClosureInfo>,
) -> Result<(), Vec<String>> {
    if stack.iter().any(|entry| entry == slug) {
        let start = stack.iter().position(|entry| entry == slug).unwrap_or(0);
        let mut path: Vec<String> = stack[start..].to_vec();
        path.push(slug.to_string());
        return Err(path);
    }
    if visited.contains(slug) {
        if let Some(existing) = closure.get_mut(slug) {
            if depth < existing.depth {
                existing.depth = depth;
                existing.explicit = explicit || existing.explicit;
            }
        }
        return Ok(());
    }
    visited.insert(slug.to_string());
    stack.push(slug.to_string());
    let section = match by_slug.get(slug) {
        Some(section) => *section,
        None => {
            stack.pop();
            return Ok(());
        }
    };
    let entry = closure.entry(slug.to_string()).or_insert(ClosureInfo {
        depth,
        explicit: false,
    });
    if depth < entry.depth || (depth == entry.depth && explicit && !entry.explicit) {
        entry.depth = depth;
        entry.explicit = explicit || entry.explicit;
    }
    for dep in &section.requires {
        if by_slug.contains_key(dep.as_str()) {
            visit(dep, depth + 1, false, by_slug, visited, stack, closure)?;
        }
    }
    stack.pop();
    Ok(())
}

/// The requested slugs closed over their `requires`, ordered the way the
/// composer needs: explicitly requested first, shallower dependencies next,
/// request order within a tier and alphabetical for what nobody requested.
pub(crate) fn resolve_sections<'a>(
    requested: &[String],
    sections: &'a [SkillSection],
) -> Result<Vec<&'a SkillSection>, ResolveError> {
    let by_slug: HashMap<&str, &'a SkillSection> =
        sections.iter().map(|s| (s.slug.as_str(), s)).collect();
    for slug in requested {
        if !by_slug.contains_key(slug.as_str()) {
            return Err(ResolveError::UnknownSlug(slug.clone()));
        }
    }
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    let mut closure: HashMap<String, ClosureInfo> = HashMap::new();
    for slug in requested {
        visit(
            slug,
            0,
            true,
            &by_slug,
            &mut visited,
            &mut stack,
            &mut closure,
        )
        .map_err(ResolveError::Cycle)?;
    }
    let requested_order: HashMap<&str, usize> = {
        let mut order = HashMap::new();
        for (index, slug) in requested.iter().enumerate() {
            order.entry(slug.as_str()).or_insert(index);
        }
        order
    };
    let mut nodes: Vec<(&String, &ClosureInfo)> = closure.iter().collect();
    nodes.sort_by(|left, right| {
        if left.1.explicit != right.1.explicit {
            return if left.1.explicit {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if left.1.depth != right.1.depth {
            return left.1.depth.cmp(&right.1.depth);
        }
        match (
            requested_order.get(left.0.as_str()),
            requested_order.get(right.0.as_str()),
        ) {
            (Some(left), Some(right)) => left.cmp(right),
            _ => left.0.cmp(right.0),
        }
    });
    Ok(nodes
        .into_iter()
        .map(|(slug, _)| {
            *by_slug
                .get(slug.as_str())
                .expect("closure holds known slugs")
        })
        .collect())
}
