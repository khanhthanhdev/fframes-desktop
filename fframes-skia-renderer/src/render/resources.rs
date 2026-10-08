use fframes::usvgr;
use std::collections::HashMap;

struct Entry<T> {
    value: T,
    bytes: usize,
}

/// Two generations, each bounded by entry count and estimated bytes.
/// Callers must check source equality on cache hits.
pub(super) struct ResourceCache<T, const BUDGET: usize> {
    current: HashMap<u64, Entry<T>>,
    previous: HashMap<u64, Entry<T>>,
    bytes: usize,
    capacity: usize,
    budget: usize,
}

impl<T, const BUDGET: usize> Default for ResourceCache<T, BUDGET> {
    fn default() -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
            bytes: 0,
            capacity: usize::MAX,
            budget: BUDGET,
        }
    }
}

impl<T, const BUDGET: usize> ResourceCache<T, BUDGET> {
    pub(super) fn with_limits(capacity: usize, budget: usize) -> Self {
        Self {
            capacity,
            budget,
            ..Default::default()
        }
    }

    pub(super) fn begin_frame(&mut self) {
        std::mem::swap(&mut self.current, &mut self.previous);
        self.current.clear();
        self.bytes = 0;
    }

    pub(super) fn get(&mut self, key: u64) -> Option<&T> {
        if !self.current.contains_key(&key)
            && self.current.len() < self.capacity
            && self
                .previous
                .get(&key)
                .is_some_and(|entry| entry.bytes <= self.budget.saturating_sub(self.bytes))
        {
            let entry = self.previous.remove(&key)?;
            self.bytes += entry.bytes;
            self.current.insert(key, entry);
        }
        self.current
            .get(&key)
            .or_else(|| self.previous.get(&key))
            .map(|entry| &entry.value)
    }

    pub(super) fn insert_with(&mut self, key: u64, bytes: usize, build: impl FnOnce() -> T) {
        let old = self.current.get(&key).map_or(0, |entry| entry.bytes);
        if (self.current.contains_key(&key) || self.current.len() < self.capacity)
            && bytes <= self.budget.saturating_sub(self.bytes - old)
        {
            self.bytes = self.bytes - old + bytes;
            self.current.insert(
                key,
                Entry {
                    value: build(),
                    bytes,
                },
            );
        }
    }
}

pub(super) struct Geometry {
    pub source: usvgr::tiny_skia_path::Path,
    pub path: skia_safe::Path,
}

pub(super) struct Fill {
    pub source: usvgr::Fill,
    pub anti_alias: bool,
    pub paint: skia_safe::Paint,
}

pub(super) struct Stroke {
    pub source: usvgr::Stroke,
    pub anti_alias: bool,
    pub paint: skia_safe::Paint,
}

pub(super) fn same_paint(a: &usvgr::Paint, b: &usvgr::Paint) -> bool {
    match (a, b) {
        (usvgr::Paint::Color(a), usvgr::Paint::Color(b)) => a == b,
        (usvgr::Paint::LinearGradient(a), usvgr::Paint::LinearGradient(b)) => {
            (a.x1(), a.y1(), a.x2(), a.y2()) == (b.x1(), b.y1(), b.x2(), b.y2())
                && same_gradient(a, b)
        }
        (usvgr::Paint::RadialGradient(a), usvgr::Paint::RadialGradient(b)) => {
            (a.cx(), a.cy(), a.r(), a.fx(), a.fy()) == (b.cx(), b.cy(), b.r(), b.fx(), b.fy())
                && same_gradient(a, b)
        }
        _ => false, // Patterns may change between frames.
    }
}

fn same_gradient(a: &usvgr::BaseGradient, b: &usvgr::BaseGradient) -> bool {
    a.transform() == b.transform()
        && a.spread_method() == b.spread_method()
        && a.stops().len() == b.stops().len()
        && a.stops().iter().zip(b.stops()).all(|(a, b)| {
            (a.offset(), a.opacity(), a.color()) == (b.offset(), b.opacity(), b.color())
        })
}

impl Fill {
    pub(super) fn matches(&self, fill: &usvgr::Fill, anti_alias: bool) -> bool {
        self.anti_alias == anti_alias
            && self.source.opacity() == fill.opacity()
            && self.source.rule() == fill.rule()
            && same_paint(self.source.paint(), fill.paint())
    }
}

impl Stroke {
    pub(super) fn matches(&self, stroke: &usvgr::Stroke, anti_alias: bool) -> bool {
        self.anti_alias == anti_alias
            && self.source.opacity() == stroke.opacity()
            && self.source.width() == stroke.width()
            && self.source.miterlimit() == stroke.miterlimit()
            && self.source.dashoffset() == stroke.dashoffset()
            && self.source.linecap() == stroke.linecap()
            && self.source.linejoin() == stroke.linejoin()
            && self.source.dasharray() == stroke.dasharray()
            && same_paint(self.source.paint(), stroke.paint())
    }
}

pub(super) fn paint_bytes(paint: &usvgr::Paint) -> usize {
    let stops = match paint {
        usvgr::Paint::LinearGradient(gradient) => gradient.stops().len(),
        usvgr::Paint::RadialGradient(gradient) => gradient.stops().len(),
        _ => 0,
    };
    256 + stops * std::mem::size_of::<usvgr::Stop>() * 2
}

#[cfg(test)]
mod tests {
    use super::ResourceCache;

    #[test]
    fn entry_and_byte_limits_survive_generation_changes() {
        let mut cache = ResourceCache::<_, 100>::with_limits(2, 10);
        cache.insert_with(1, 4, || "one");
        cache.insert_with(2, 4, || "two");
        cache.insert_with(3, 1, || panic!("entry limit must reject insertion"));
        assert!(cache.get(3).is_none());
        cache.insert_with(1, 7, || panic!("replacement must obey byte limit"));
        assert_eq!(cache.get(1), Some(&"one"));
        cache.insert_with(1, 6, || "replacement");
        assert_eq!(cache.get(1), Some(&"replacement"));

        cache.begin_frame();
        cache.insert_with(3, 7, || "three");
        assert_eq!(cache.get(1), Some(&"replacement"));
        assert_eq!(cache.current.len(), 1);
        assert_eq!(cache.bytes, 7);
        cache.begin_frame();
        assert!(cache.get(1).is_none());
        assert_eq!(cache.get(3), Some(&"three"));
    }

    #[test]
    fn zero_limits_disable_geometry_retention() {
        for (capacity, bytes) in [(0, 10), (10, 0)] {
            let mut cache = ResourceCache::<_, 100>::with_limits(capacity, bytes);
            cache.insert_with(1, 1, || panic!("disabled cache must not allocate"));
            assert!(cache.get(1).is_none());
        }
    }
}
