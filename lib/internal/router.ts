import type { Handler, RouteMiddleware, StringMap } from "../types";
import { frameworkError, ErrorCode } from "./errors";

export const EMPTY_OBJECT = Object.freeze({});

// A node in our radix tree. Each one can hold up to three kinds of children:
// an exact static segment, a single ":param" placeholder, or a tail "*"
// wildcard. The handler and middleware here belong to the route whose path
// ends at this node, if any.
//
// Param names are not stored on the tree edges. We capture values positionally
// as we walk, and zip them with the param names attached to whichever leaf we
// land on. That lets two routes share the same param slot in the tree even
// when they use different names, like "/:id/profile" and "/:username/settings".
interface RadixNode {
  staticChildren: Map<string, RadixNode>;
  paramChild?: RadixNode;
  wildcardChild?: WildcardLeaf;
  handler?: Handler;
  middleware?: RouteMiddleware[];
  // Names of params captured along the path to this leaf, in order. Only set
  // on nodes that own a handler.
  paramNames?: string[];
}

interface WildcardLeaf {
  handler: Handler;
  middleware: RouteMiddleware[];
  // Names of params captured before reaching this wildcard, in order.
  paramNames: string[];
}

export interface RouteMatch {
  middleware: RouteMiddleware[];
  handler: Handler;
  params: StringMap;
}

function createNode(): RadixNode {
  return { staticChildren: new Map() };
}

// We keep one radix tree per HTTP method so different methods can safely
// share a path shape. POST /comments/:pageId and PUT /comments/:id can
// coexist without conflict because they live in separate trees.
export class Router {
  #treesByMethod: Map<string, RadixNode> = new Map();

  #staticRouterPerMethod: Map<string, Map<string, RouteMatch>> = new Map();

  add(
    method: string,
    path: string,
    middleware: RouteMiddleware[],
    handler: Handler
  ) {
    const methodKey = method.toLowerCase();
    let root = this.#treesByMethod.get(methodKey);
    if (!root) {
      root = createNode();
      this.#treesByMethod.set(methodKey, root);
    }

    const segments = splitPath(path);
    const paramNames: string[] = [];
    let currentNode = root;

    for (let i = 0; i < segments.length; i++) {
      const segment = segments[i];
      const isLastSegment = i === segments.length - 1;

      // Named wildcards like "*name" are not a thing here. Only a bare "*"
      // is allowed, and only as the very last segment.
      if (segment.length > 1 && segment.startsWith("*")) {
        throw frameworkError(
          `Invalid route "${path}": named wildcards (e.g. "*name") are not supported. Use a plain "*" at the end of the path.`,
          this.add,
          ErrorCode.INVALID_ROUTE
        );
      }

      // A "*" segment installs a tail wildcard on the current node. After
      // that there's nothing more to walk, so we register and bail out.
      if (segment === "*") {
        if (!isLastSegment) {
          throw frameworkError(
            `Invalid route "${path}": "*" is only allowed as the final path segment.`,
            this.add,
            ErrorCode.INVALID_ROUTE
          );
        }
        if (currentNode.wildcardChild) {
          throw frameworkError(
            `Duplicate route: ${method.toUpperCase()} ${path}`,
            this.add,
            ErrorCode.DUPLICATE_ROUTE
          );
        }
        currentNode.wildcardChild = { handler, middleware, paramNames };
        return;
      }

      // A ":name" segment walks into the param branch at this depth, or
      // creates one. The name is collected positionally and resolved later
      // at the leaf, so two routes can disagree on the param name here as
      // long as their paths diverge before the leaf.
      if (segment.startsWith(":")) {
        const paramName = segment.slice(1);
        if (!paramName) {
          throw frameworkError(
            `Invalid route "${path}": empty parameter name.`,
            this.add,
            ErrorCode.INVALID_ROUTE
          );
        }
        paramNames.push(paramName);
        if (!currentNode.paramChild) {
          currentNode.paramChild = createNode();
        }
        currentNode = currentNode.paramChild;
        continue;
      }

      // Plain static segment. Walk into the existing child or create a new one.
      let staticChild = currentNode.staticChildren.get(segment);
      if (!staticChild) {
        staticChild = createNode();
        currentNode.staticChildren.set(segment, staticChild);
      }
      currentNode = staticChild;
    }

    // We have consumed every segment of the path. The terminal node is where
    // the handler gets attached. If something is already attached here, the
    // user registered this exact path twice.
    if (currentNode.handler) {
      throw frameworkError(
        `Duplicate route: ${method.toUpperCase()} ${path}`,
        this.add,
        ErrorCode.DUPLICATE_ROUTE
      );
    }
    currentNode.handler = handler;
    currentNode.middleware = middleware;
    currentNode.paramNames = paramNames;

    const is_static = !path.includes(":") && !path.includes("*");
    if (is_static) {
      let staticMethodRoute = this.#staticRouterPerMethod.get(methodKey);
      // create a new map for the Method
      if (!staticMethodRoute) {
        staticMethodRoute = new Map();
        this.#staticRouterPerMethod.set(methodKey, staticMethodRoute);
      }
      staticMethodRoute.set(path, {
        middleware,
        handler,
        params: EMPTY_OBJECT
      });
      return;
    }
  }

  find(method: string, path: string): RouteMatch | null {
    const methodKey = method.toLowerCase();

    const staticMap = this.#staticRouterPerMethod.get(methodKey);
    if (staticMap !== undefined) {
      const cachedResult = staticMap.get(path);
      if (cachedResult !== undefined) return cachedResult;
    }

    const root = this.#treesByMethod.get(methodKey);
    if (!root) return null;

    const segments = splitPath(path);
    return matchSegments(root, segments, 0, []);
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
// get zipped in at the terminal leaf, using the paramNames stored alongside
// the handler. That way the same captured value can be called "id" on one
// route and "username" on another without the tree caring.
function matchSegments(
  node: RadixNode,
  segments: string[],
  segmentIndex: number,
  capturedValues: string[]
): RouteMatch | null {
  // Out of segments to walk. If this node has a handler, that's our match.
  // Otherwise let a wildcard at this depth catch the empty remainder so
  // routes like "/foo/*" still match a request to "/foo".
  if (segmentIndex === segments.length) {
    if (node.handler) {
      return {
        middleware: node.middleware!,
        handler: node.handler,
        params: zipParams(node.paramNames!, capturedValues)
      };
    }
    if (node.wildcardChild) {
      return {
        middleware: node.wildcardChild.middleware,
        handler: node.wildcardChild.handler,
        params: zipParams(node.wildcardChild.paramNames, capturedValues)
      };
    }
    return null;
  }

  const segment = segments[segmentIndex];

  // Try the exact static child first. Exact matches always win.
  const staticChild = node.staticChildren.get(segment);
  if (staticChild) {
    const foundMatch = matchSegments(
      staticChild,
      segments,
      segmentIndex + 1,
      capturedValues
    );
    if (foundMatch) return foundMatch;
  }

  // Then try the param branch. We push the captured value before recursing
  // and pop it back off if the recursion fails, so any sibling branch (or the
  // caller unwinding above us) sees a clean capture list.
  if (node.paramChild) {
    capturedValues.push(safeDecode(segment));
    const foundMatch = matchSegments(
      node.paramChild,
      segments,
      segmentIndex + 1,
      capturedValues
    );
    if (foundMatch) return foundMatch;
    capturedValues.pop();
  }

  // Last resort. A wildcard at this node swallows whatever segments remain.
  if (node.wildcardChild) {
    return {
      middleware: node.wildcardChild.middleware,
      handler: node.wildcardChild.handler,
      params: zipParams(node.wildcardChild.paramNames, capturedValues)
    };
  }

  return null;
}

function zipParams(names: string[], values: string[]): StringMap {
  const params: StringMap = {};
  for (let i = 0; i < names.length; i++) {
    params[names[i]] = values[i];
  }
  return params;
}

// Decode a URL segment without ever throwing. Malformed percent encoding is
// rare but it does happen in the wild. Falling back to the raw segment keeps
// the request matchable instead of blowing up before the handler runs.
// Example: safeDecode("a%20b%2Fc") returns "a b/c", while safeDecode("a%ZZb") returns "a%ZZb".
function safeDecode(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

// Split a URL path into segments with no leading slash. We treat "" and "/"
// the same way: zero segments, meaning the root of the tree.
// Example: "/a/b/c" becomes ["a", "b", "c"]
function splitPath(path: string): string[] {
  if (path === "" || path === "/") return [];
  const withoutLeadingSlash = path.startsWith("/") ? path.slice(1) : path;
  return withoutLeadingSlash.split("/");
}
