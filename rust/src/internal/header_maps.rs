// The header maps of a worker. A request that is done gives its map back, and a response takes
// one when it gets its first header (CpeakResponse::headers_mut()), so maps get reused instead
// of allocated.

use std::cell::RefCell;

use hyper::HeaderMap;

// How many maps a worker keeps, and how big a map can be to be kept
const KEPT: usize = 64;
const MAX_CAPACITY: usize = 64;

thread_local! {
    static MAPS: RefCell<Vec<HeaderMap>> = const { RefCell::new(Vec::new()) };
}

// An empty map, for a response
pub(crate) fn take() -> HeaderMap {
    MAPS.try_with(|maps| maps.try_borrow_mut().ok()?.pop())
        .ok()
        .flatten()
        .unwrap_or_default()
}

// The map of a request that is done
pub(crate) fn give_back(mut map: HeaderMap) {
    let capacity = map.capacity();
    if capacity == 0 || capacity > MAX_CAPACITY {
        return;
    }
    map.clear();
    let _ = MAPS.try_with(|maps| {
        if let Ok(mut maps) = maps.try_borrow_mut() {
            if maps.len() < KEPT {
                maps.push(map);
            }
        }
    });
}
