// The HTML App bridge shim (docs/bridge.md).
//
// Injected at document-start, before any page script runs. Everything the page can reach goes
// through here, and this file cannot widen what the manifest granted: the host re-checks every
// call, and modules the manifest did not ask for are never written onto the global at all, so
// `typeof htmlapp.fs === "undefined"` is a truthful feature test rather than a disabled stub.
(function () {
  "use strict";

  var RUNTIME_VERSION = "__HTMLAPP_VERSION__";
  var PERMISSIONS = __HTMLAPP_PERMISSIONS__;
  var MODULES = __HTMLAPP_MODULES__;
  var HEADLESS = __HTMLAPP_HEADLESS__;
  var FORMAT = "__HTMLAPP_FORMAT__";

  var pending = new Map(); // id -> {resolve, reject}
  var streams = new Map(); // id -> stream controller
  var handlers = new Map(); // event name -> Map(token -> handler)
  var nextId = 1;
  var nextToken = 1;

  function post(message) {
    // wry exposes exactly one channel to the host; everything is multiplexed over it.
    window.ipc.postMessage(JSON.stringify(message));
  }

  function rpcError(error) {
    var err = new Error((error && error.message) || "bridge call failed");
    err.name = "HtmlAppError";
    err.code = (error && error.code) || "internal";
    if (error && error.data !== undefined) err.data = error.data;
    return err;
  }

  // --- request/response ---------------------------------------------------

  function invoke(method, params) {
    var id = nextId++;
    return new Promise(function (resolve, reject) {
      pending.set(id, { resolve: resolve, reject: reject });
      post({ t: "invoke", id: id, method: method, params: params === undefined ? null : params });
    });
  }

  // --- streams ------------------------------------------------------------
  //
  // Chunks can arrive faster than the consumer's loop body runs, so they queue rather than being
  // dropped. `cancel()` and breaking out of a `for await` both tell the host to stop producing,
  // which is what keeps a cancelled `fs.tail` from leaking an inotify watch for the session.

  function stream(method, params) {
    var id = nextId++;
    var queue = [];
    var waiters = [];
    var finished = false;
    var failure = null;
    var cancelled = false;

    function settle() {
      while (waiters.length && (queue.length || finished || failure)) {
        var waiter = waiters.shift();
        if (queue.length) waiter.resolve({ value: queue.shift(), done: false });
        else if (failure) waiter.reject(rpcError(failure));
        else waiter.resolve({ value: undefined, done: true });
      }
    }

    streams.set(id, {
      chunk: function (value) {
        queue.push(value);
        settle();
      },
      end: function () {
        finished = true;
        streams.delete(id);
        settle();
      },
      fail: function (error) {
        failure = error;
        streams.delete(id);
        settle();
      },
    });

    post({ t: "streamStart", id: id, method: method, params: params === undefined ? null : params });

    function cancel() {
      if (cancelled) return;
      cancelled = true;
      if (!finished && !failure) post({ t: "streamCancel", id: id });
      finished = true;
      streams.delete(id);
      settle();
    }

    var iterator = {
      next: function () {
        if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
        if (failure) return Promise.reject(rpcError(failure));
        if (finished) return Promise.resolve({ value: undefined, done: true });
        return new Promise(function (resolve, reject) {
          waiters.push({ resolve: resolve, reject: reject });
        });
      },
      // Called when the consumer breaks out of `for await`.
      return: function () {
        cancel();
        return Promise.resolve({ value: undefined, done: true });
      },
      throw: function (e) {
        cancel();
        return Promise.reject(e);
      },
    };
    iterator[Symbol.asyncIterator] = function () {
      return iterator;
    };
    iterator.cancel = cancel;
    return iterator;
  }

  // --- events -------------------------------------------------------------

  function on(event, handler) {
    if (typeof handler !== "function") throw new TypeError("handler must be a function");
    var token = nextToken++;
    if (!handlers.has(event)) {
      handlers.set(event, new Map());
      // Only tell the host about the first subscriber; it does not need a message per handler.
      post({ t: "subscribe", id: token, event: event });
    }
    handlers.get(event).set(token, handler);

    return function off() {
      var forEvent = handlers.get(event);
      if (!forEvent) return;
      forEvent.delete(token);
      if (forEvent.size === 0) {
        handlers.delete(event);
        post({ t: "unsubscribe", id: token });
      }
    };
  }

  function once(event, handler) {
    var off = on(event, function (payload) {
      off();
      handler(payload);
    });
    return off;
  }

  // --- host → page dispatch ------------------------------------------------

  function dispatch(raw) {
    var message;
    try {
      message = typeof raw === "string" ? JSON.parse(raw) : raw;
    } catch (e) {
      console.error("[htmlapp] malformed message from host", e);
      return;
    }

    switch (message.t) {
      case "result": {
        var entry = pending.get(message.id);
        if (entry) {
          pending.delete(message.id);
          entry.resolve(message.value);
        }
        break;
      }
      case "error": {
        var failed = pending.get(message.id);
        if (failed) {
          pending.delete(message.id);
          failed.reject(rpcError(message.error));
        }
        break;
      }
      case "chunk": {
        var chunked = streams.get(message.id);
        if (chunked) chunked.chunk(message.value);
        break;
      }
      case "end": {
        var ended = streams.get(message.id);
        if (ended) ended.end();
        break;
      }
      case "streamError": {
        var broken = streams.get(message.id);
        if (broken) broken.fail(message.error);
        break;
      }
      case "event": {
        var forEvent = handlers.get(message.event);
        if (forEvent) {
          // Copy first: a handler is allowed to unsubscribe itself.
          Array.prototype.forEach.call(Array.from(forEvent.values()), function (handler) {
            try {
              handler(message.payload);
            } catch (e) {
              console.error("[htmlapp] handler for " + message.event + " threw", e);
            }
          });
        }
        break;
      }
      default:
        console.warn("[htmlapp] unknown message type", message.t);
    }
  }

  // --- module construction -------------------------------------------------
  //
  // The API catalog: "Absent permission, the module is not injected at all — the property does not exist, so
  // feature detection works naturally."

  function buildModule(name, spec) {
    var module = {};
    Object.keys(spec).forEach(function (methodName) {
      var kind = spec[methodName];
      var qualified = name + "." + methodName;
      module[methodName] =
        kind === "stream"
          ? function (params) {
              return stream(qualified, params);
            }
          : function (params) {
              return invoke(qualified, params);
            };
    });
    return Object.freeze(module);
  }

  // --- native views (docs/bridge.md) ---------------------------------------------------
  //
  // The custom element is a transparent placeholder that takes part in normal HTML layout. A
  // ResizeObserver reports its viewport rect to the host, which paints a real GPUI element there,
  // above the page, and routes input that lands inside it.

  var views = new Map();

  function viewHandle(id) {
    if (!views.has(id)) {
      views.set(
        id,
        Object.freeze({
          id: id,
          write: function (data) {
            return invoke("view.write", { id: id, data: data });
          },
          call: function (method, params) {
            return invoke("view.call", { id: id, method: method, params: params });
          },
          set: function (props) {
            return invoke("view.set", { id: id, props: props });
          },
          on: function (event, handler) {
            return on("view:" + id + ":" + event, handler);
          },
        })
      );
    }
    return views.get(id);
  }

  function registerViewElement() {
    if (typeof customElements === "undefined" || customElements.get("htmlapp-view")) return;

    var HtmlAppView = function () {
      return Reflect.construct(HTMLElement, [], HtmlAppView);
    };
    HtmlAppView.prototype = Object.create(HTMLElement.prototype);
    HtmlAppView.prototype.constructor = HtmlAppView;
    Object.setPrototypeOf(HtmlAppView, HTMLElement);

    HtmlAppView.prototype.connectedCallback = function () {
      var element = this;
      var id = element.id || "view-" + nextId++;
      element.__htmlappViewId = id;
      // The placeholder must never paint over the native view the host draws in its place.
      element.style.visibility = "hidden";

      invoke("view.create", {
        id: id,
        kind: element.getAttribute("kind") || "terminal",
        options: parseViewOptions(element),
      }).catch(function (e) {
        console.error("[htmlapp] could not create view " + id, e);
      });

      var report = function () {
        var rect = element.getBoundingClientRect();
        invoke("view.layout", {
          id: id,
          x: rect.left,
          y: rect.top,
          width: rect.width,
          height: rect.height,
          scale: window.devicePixelRatio || 1,
        }).catch(function () {});
      };

      element.__htmlappReport = report;
      element.__htmlappObserver = new ResizeObserver(report);
      element.__htmlappObserver.observe(element);
      // Scrolling and reflow move the element without resizing it.
      window.addEventListener("scroll", report, true);
      window.addEventListener("resize", report);
      report();
    };

    HtmlAppView.prototype.disconnectedCallback = function () {
      var id = this.__htmlappViewId;
      if (this.__htmlappObserver) this.__htmlappObserver.disconnect();
      if (this.__htmlappReport) {
        window.removeEventListener("scroll", this.__htmlappReport, true);
        window.removeEventListener("resize", this.__htmlappReport);
      }
      if (id) {
        views.delete(id);
        invoke("view.destroy", { id: id }).catch(function () {});
      }
    };

    customElements.define("htmlapp-view", HtmlAppView);
  }

  function parseViewOptions(element) {
    var raw = element.getAttribute("options");
    if (!raw) return {};
    try {
      return JSON.parse(raw);
    } catch (e) {
      console.warn("[htmlapp] <htmlapp-view options> is not valid JSON", e);
      return {};
    }
  }

  // --- the global ----------------------------------------------------------

  var htmlapp = {
    invoke: invoke,
    stream: stream,
    on: on,
    once: once,
    version: RUNTIME_VERSION,
    permissions: Object.freeze(PERMISSIONS),
    view: viewHandle,
  };

  Object.keys(MODULES).forEach(function (name) {
    htmlapp[name] = buildModule(name, MODULES[name]);
  });

  // Headless mode: headless mode makes a .hta a legitimate participant in a shell pipeline.
  if (HEADLESS) {
    htmlapp.stdin = Object.freeze({
      read: function () {
        return invoke("stdio.read", null);
      },
      lines: function () {
        return stream("stdio.lines", null);
      },
    });
    htmlapp.stdout = Object.freeze({
      write: function (data) {
        return invoke("stdio.write", { data: data });
      },
      writeLine: function (data) {
        return invoke("stdio.write", { data: String(data) + "\n" });
      },
    });
    htmlapp.stderr = Object.freeze({
      write: function (data) {
        return invoke("stdio.writeErr", { data: data });
      },
    });
    // Headless mode's `--format json`. A hint from the invoker about what shape of output is wanted;
    // The document decides what to do with it.
    htmlapp.format = FORMAT || null;

    htmlapp.exit = function (code) {
      return invoke("stdio.exit", { code: code === undefined ? 0 : code });
    };

    // A headless document exits when it says so (docs/bridge.md) — but a document that throws never gets to
    // say so, and would otherwise hang whatever shell pipeline it is part of, forever. An uncaught
    // error is reported on stderr and exits non-zero, which is what any other pipeline tool does.
    var reportFatal = function (kind, error) {
      var detail = (error && (error.stack || error.message)) || String(error);
      var report = htmlapp.stderr.write("htmlapp: uncaught " + kind + ": " + detail + "\n");
      // Exit regardless of whether the report itself made it out.
      report.then(
        function () { htmlapp.exit(1); },
        function () { htmlapp.exit(1); }
      );
    };

    window.addEventListener("error", function (event) {
      reportFatal("error", event.error || event.message);
    });
    window.addEventListener("unhandledrejection", function (event) {
      reportFatal("promise rejection", event.reason);
    });
  }

  // The bridge transport: "Object.freeze(htmlapp)". Nothing in the page can add a module that was not granted,
  // or swap `invoke` for something that lies to the rest of the page about what it called.
  Object.freeze(htmlapp);
  Object.defineProperty(window, "htmlapp", {
    value: htmlapp,
    writable: false,
    configurable: false,
    enumerable: true,
  });

  // The host's only entry point into the page.
  Object.defineProperty(window, "__htmlapp_dispatch", {
    value: dispatch,
    writable: false,
    configurable: false,
    enumerable: false,
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", registerViewElement);
  } else {
    registerViewElement();
  }

  post({ t: "invoke", id: 0, method: "runtime.ready", params: null });
})();
