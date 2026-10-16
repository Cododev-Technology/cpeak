use crate::internal::errors::{framework_error, throw, ErrorCode};
use crate::internal::uri::decode_uri_component;
use crate::types::{DynHandler, DynMiddleware, StringMap};

// A node in our radix tree. Each one can hold up to three kinds of children:
// an exact static segment, a single ":param" placeholder, or a tail "*"
// wildcard. The handler and middleware here belong to the route whose path
// ends at this node, if any.
//
// Param names are not stored on the tree edges. We capture values positionally
// as we walk, and zip them with the param names attached to whichever leaf we
// land on. That lets two routes share the same param slot in the tree even
// when they use different names, like "/:id/profile" and "/:username/settings".
struct RadixNode {
    // Sorted by segment, so we can find a child with a binary search (no hashing needed)
    static_children: Vec<(String, RadixNode)>,
    param_child: Option<Box<RadixNode>>,
    wildcard_child: Option<WildcardLeaf>,
    handler: Option<Box<dyn DynHandler>>,
    middleware: Vec<Box<dyn DynMiddleware>>,
    // Names of params captured along the path to this leaf, in order. Only set
    // on nodes that own a handler.
    param_names: Vec<String>,
}

struct WildcardLeaf {
    handler: Box<dyn DynHandler>,
    middleware: Vec<Box<dyn DynMiddleware>>,
    // Names of params captured before reaching this wildcard, in order.
    param_names: Vec<String>,
}

pub(crate) struct RouteMatch<'a> {
    pub(crate) middleware: &'a [Box<dyn DynMiddleware>],
    pub(crate) handler: &'a dyn DynHandler,
    pub(crate) params: StringMap,
}

// What the tree walk finds: the route, and the names of its params
struct Leaf<'a> {
    middleware: &'a [Box<dyn DynMiddleware>],
    handler: &'a dyn DynHandler,
    param_names: &'a [String],
}

fn create_node() -> RadixNode {
    RadixNode {
        static_children: Vec::new(),
        param_child: None,
        wildcard_child: None,
        handler: None,
        middleware: Vec::new(),
        param_names: Vec::new(),
    }
}

// We keep one radix tree per HTTP method so different methods can safely
// share a path shape. POST /comments/:pageId and PUT /comments/:id can
// coexist without conflict because they live in separate trees.
pub(crate) struct Router {
    // (lower-cased method, tree). There are only a handful of methods, so a list is the fastest.
    trees_by_method: Vec<(String, RadixNode)>,
}

impl RadixNode {
    fn static_child(&self, segment: &str) -> Option<&RadixNode> {
        self.static_children
            .binary_search_by(|(child, _)| child.as_str().cmp(segment))
            .ok()
            .map(|index| &self.static_children[index].1)
    }

    fn static_child_mut(&mut self, segment: &str) -> &mut RadixNode {
        let index = match self
            .static_children
            .binary_search_by(|(child, _)| child.as_str().cmp(segment))
        {
            Ok(index) => index,
            Err(index) => {
                self.static_children
                    .insert(index, (segment.to_string(), create_node()));
                index
            }
        };
        &mut self.static_children[index].1
    }
}

impl Router {
    pub(crate) fn new() -> Self {
        Self {
            trees_by_method: Vec::new(),
        }
    }

    pub(crate) fn add(
        &mut self,
        method: &str,
        path: &str,
        middleware: Vec<Box<dyn DynMiddleware>>,
        handler: Box<dyn DynHandler>,
    ) {
        let method_key = method.to_lowercase();
        let tree_index = match self
            .trees_by_method
            .iter()
            .position(|(tree_method, _)| *tree_method == method_key)
        {
            Some(index) => index,
            None => {
                self.trees_by_method.push((method_key, create_node()));
                self.trees_by_method.len() - 1
            }
        };
        let root = &mut self.trees_by_method[tree_index].1;

        let segments: Vec<&str> =
            split_path(path).map_or_else(Vec::new, |path| path.split('/').collect());
        let mut param_names: Vec<String> = Vec::new();
        let mut current_node = root;

        for (i, segment) in segments.iter().enumerate() {
            let is_last_segment = i == segments.len() - 1;

            // Named wildcards like "*name" are not a thing here. Only a bare "*"
            // is allowed, and only as the very last segment.
            if segment.len() > 1 && segment.starts_with('*') {
                throw(framework_error(
                    format!("Invalid route \"{path}\": named wildcards (e.g. \"*name\") are not supported. Use a plain \"*\" at the end of the path."),
                    ErrorCode::InvalidRoute,
                    None,
                    false,
                ));
            }

            // A "*" segment installs a tail wildcard on the current node. After
            // that there's nothing more to walk, so we register and bail out.
            if *segment == "*" {
                if !is_last_segment {
                    throw(framework_error(
                        format!("Invalid route \"{path}\": \"*\" is only allowed as the final path segment."),
                        ErrorCode::InvalidRoute,
                        None,
                        false,
                    ));
                }
                if current_node.wildcard_child.is_some() {
                    throw(framework_error(
                        format!("Duplicate route: {} {path}", method.to_uppercase()),
                        ErrorCode::DuplicateRoute,
                        None,
                        false,
                    ));
                }
                current_node.wildcard_child = Some(WildcardLeaf {
                    handler,
                    middleware,
                    param_names,
                });
                return;
            }

            // A ":name" segment walks into the param branch at this depth, or
            // creates one. The name is collected positionally and resolved later
            // at the leaf, so two routes can disagree on the param name here as
            // long as their paths diverge before the leaf.
            if let Some(param_name) = segment.strip_prefix(':') {
                if param_name.is_empty() {
                    throw(framework_error(
                        format!("Invalid route \"{path}\": empty parameter name."),
                        ErrorCode::InvalidRoute,
                        None,
                        false,
                    ));
                }
                param_names.push(param_name.to_string());
                current_node = current_node
                    .param_child
                    .get_or_insert_with(|| Box::new(create_node()));
                continue;
            }

            // Plain static segment. Walk into the existing child or create a new one.
            current_node = current_node.static_child_mut(segment);
        }

        // We have consumed every segment of the path. The terminal node is where
        // the handler gets attached. If something is already attached here, the
        // user registered this exact path twice.
        if current_node.handler.is_some() {
            throw(framework_error(
                format!("Duplicate route: {} {path}", method.to_uppercase()),
                ErrorCode::DuplicateRoute,
                None,
                false,
            ));
        }
        current_node.handler = Some(handler);
        current_node.middleware = middleware;
        current_node.param_names = param_names;
    }

    // The method can come in any case ("GET" or "get"), like in the Node.js version, where it's lower-cased first
    pub(crate) fn find(&self, method: &str, path: &str) -> Option<RouteMatch<'_>> {
        let (_, root) = self
            .trees_by_method
            .iter()
            .find(|(tree_method, _)| tree_method.eq_ignore_ascii_case(method))?;

        let mut captured_values = Vec::new();
        let leaf = match_segments(root, split_path(path), &mut captured_values)?;
        Some(RouteMatch {
            middleware: leaf.middleware,
            handler: leaf.handler,
            params: zip_params(leaf.param_names, &captured_values),
        })
    }
}

// Walk the tree one segment at a time, always trying static before param
// before wildcard. That ordering is where our precedence rules come from:
// static beats param beats wildcard. Because each branch is tried in turn
// and recursion lets us unwind a failed path, the matcher also backtracks.
// If the static branch dead-ends deeper down, we come back up and try the
// param sibling with the same segment value.
//
// We collect captured param values positionally as we walk. The actual names
// get zipped in at the terminal leaf, using the param_names stored alongside
// the handler. That way the same captured value can be called "id" on one
// route and "username" on another without the tree caring.
//
// `segments` is the rest of the path that we haven't walked yet (None once we're out of
// segments). We split it one segment at a time as we go, instead of splitting it all up front.
fn match_segments<'a, 'p>(
    node: &'a RadixNode,
    segments: Option<&'p str>,
    captured_values: &mut Vec<&'p str>,
) -> Option<Leaf<'a>> {
    // Out of segments to walk. If this node has a handler, that's our match.
    // Otherwise let a wildcard at this depth catch the empty remainder so
    // routes like "/foo/*" still match a request to "/foo".
    let Some(segments) = segments else {
        if let Some(handler) = &node.handler {
            return Some(Leaf {
                middleware: &node.middleware,
                handler: handler.as_ref(),
                param_names: &node.param_names,
            });
        }
        if let Some(wildcard_child) = &node.wildcard_child {
            return Some(wildcard_leaf(wildcard_child));
        }
        return None;
    };

    // '/' is one byte, so we look for it byte by byte (quicker than split_once for short paths)
    let (segment, rest) = match segments.bytes().position(|byte| byte == b'/') {
        Some(at) => (&segments[..at], Some(&segments[at + 1..])),
        None => (segments, None),
    };

    // Try the exact static child first. Exact matches always win.
    if let Some(static_child) = node.static_child(segment) {
        let found_match = match_segments(static_child, rest, captured_values);
        if found_match.is_some() {
            return found_match;
        }
    }

    // Then try the param branch. We push the captured value before recursing
    // and pop it back off if the recursion fails, so any sibling branch (or the
    // caller unwinding above us) sees a clean capture list.
    if let Some(param_child) = &node.param_child {
        captured_values.push(segment);
        let found_match = match_segments(param_child, rest, captured_values);
        if found_match.is_some() {
            return found_match;
        }
        captured_values.pop();
    }

    // Last resort. A wildcard at this node swallows whatever segments remain.
    if let Some(wildcard_child) = &node.wildcard_child {
        return Some(wildcard_leaf(wildcard_child));
    }

    None
}

fn wildcard_leaf(wildcard_child: &WildcardLeaf) -> Leaf<'_> {
    Leaf {
        middleware: &wildcard_child.middleware,
        handler: wildcard_child.handler.as_ref(),
        param_names: &wildcard_child.param_names,
    }
}

// The captured values are decoded here, once we know the route matched
fn zip_params(names: &[String], values: &[&str]) -> StringMap {
    names
        .iter()
        .zip(values)
        .map(|(name, value)| (name.clone(), safe_decode(value)))
        .collect()
}

// Decode a URL segment without ever failing. Malformed percent encoding is
// rare but it does happen in the wild. Falling back to the raw segment keeps
// the request matchable instead of blowing up before the handler runs.
// Example: safe_decode("a%20b%2Fc") returns "a b/c", while safe_decode("a%ZZb") returns "a%ZZb".
fn safe_decode(segment: &str) -> String {
    decode_uri_component(segment).unwrap_or_else(|| segment.to_string())
}

// Split a URL path into segments with no leading slash. We treat "" and "/"
// the same way: zero segments, meaning the root of the tree.
// Example: "/a/b/c" becomes the segments "a", "b" and "c" (walked one by one in match_segments)
fn split_path(path: &str) -> Option<&str> {
    if path.is_empty() || path == "/" {
        return None;
    }
    Some(path.strip_prefix('/').unwrap_or(path))
}
