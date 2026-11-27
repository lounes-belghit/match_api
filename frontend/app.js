(function () {
  const h = React.createElement;
  const BASE = window.MATCHS_API_BASE || "";
  const DEFAULT_KEY = window.MATCHS_DEMO_KEY || "";

  const ENDPOINTS = [
    { group: "Health", method: "GET", path: "/healthz", label: "Liveness", params: [] },
    { group: "Health", method: "GET", path: "/readyz", label: "Readiness", params: [] },
    { group: "Metadata", method: "GET", path: "/api/v1/competitions", label: "Competitions", params: [] },
    { group: "Metadata", method: "GET", path: "/api/v1/seasons", label: "Seasons", params: [
      { name: "competition", label: "Competition", placeholder: "L1" }
    ] },
    { group: "Metadata", method: "GET", path: "/api/v1/groups", label: "Groups", params: [] },
    { group: "Discover", method: "GET", path: "/api/v1/clubs", label: "Clubs", params: [
      { name: "q", label: "Query", placeholder: "JS Kabylie" },
      { name: "limit", label: "Limit", placeholder: "20" }
    ] },
    { group: "Matches", method: "GET", path: "/api/v1/matches", label: "List matches", params: [
      { name: "competition", label: "Competition", placeholder: "L1" },
      { name: "season", label: "Season", placeholder: "lfp:6" },
      { name: "round", label: "Round", placeholder: "12" },
      { name: "status", label: "Status", placeholder: "finished" },
      { name: "from", label: "From (RFC3339)", placeholder: "2026-11-01T00:00:00Z" },
      { name: "to", label: "To (RFC3339)", placeholder: "2026-11-30T00:00:00Z" },
      { name: "club_id", label: "Club UUID", placeholder: "uuid" },
      { name: "page", label: "Page", placeholder: "1" },
      { name: "per_page", label: "Per page", placeholder: "20" }
    ] },
    { group: "Matches", method: "GET", path: "/api/v1/matches/{id}", label: "Get match", params: [
      { name: "id", label: "Match UUID", placeholder: "uuid", path: true }
    ] },
    { group: "Matches", method: "GET", path: "/api/v1/matches/{id}/events", label: "Match events", params: [
      { name: "id", label: "Match UUID", placeholder: "uuid", path: true }
    ] },
    { group: "Matches", method: "GET", path: "/api/v1/live", label: "Near live", params: [] },
    { group: "Discover", method: "GET", path: "/api/v1/standings", label: "Standings", params: [
      { name: "season", label: "Season", placeholder: "lfp:6" },
      { name: "group", label: "Group", placeholder: "Group 1" },
      { name: "competition", label: "Competition", placeholder: "L1" }
    ] },
    { group: "Discover", method: "GET", path: "/api/v1/search", label: "Search", params: [
      { name: "q", label: "Query", placeholder: "JS" },
      { name: "limit", label: "Limit", placeholder: "20" }
    ] },
    { group: "Health", method: "GET", path: "/api/v1/status", label: "Source status", params: [] }
  ];

  const GROUPS = ["Health", "Metadata", "Discover", "Matches"];
  const PRESETS = [
    { label: "Recent matches", index: 6, values: { per_page: "10" } },
    { label: "L1 standings", index: 11, values: { competition: "L1" } },
    { label: "Search JS", index: 12, values: { q: "JS", limit: "10" } },
    { label: "Near live", index: 10, values: {} },
    { label: "Source status", index: 13, values: {} }
  ];

  function buildUrl(endpoint, values) {
    let path = endpoint.path.replace(/\{(\w+)\}/g, function (_, name) {
      return encodeURIComponent(values[name] || "");
    });
    const query = new URLSearchParams();
    endpoint.params.forEach(function (param) {
      if (!param.path && values[param.name]) query.set(param.name, values[param.name]);
    });
    const suffix = query.toString();
    return BASE + path + (suffix ? "?" + suffix : "");
  }

  function curlCommand(url, key) {
    return "curl '" + url + "' \\\n  -H 'X-API-Key: " + key + "'";
  }

  function Field(props) {
    return h("div", null,
      h("label", { htmlFor: "field-" + props.param.name }, props.param.label),
      h("input", {
        id: "field-" + props.param.name,
        value: props.value || "",
        placeholder: props.param.placeholder || "",
        onChange: function (event) {
          props.onChange(props.param.name, event.target.value);
        }
      })
    );
  }

  function App() {
    const [selected, setSelected] = React.useState(6);
    const [values, setValues] = React.useState({ per_page: "10" });
    const [key, setKey] = React.useState(function () {
      try {
        return localStorage.getItem("matchs_demo_key") || DEFAULT_KEY;
      } catch (error) {
        return DEFAULT_KEY;
      }
    });
    const [showKey, setShowKey] = React.useState(false);
    const [response, setResponse] = React.useState(null);
    const [loading, setLoading] = React.useState(false);
    const [copied, setCopied] = React.useState(false);

    React.useEffect(function () {
      try {
        localStorage.setItem("matchs_demo_key", key);
      } catch (error) {
      }
    }, [key]);

    const endpoint = ENDPOINTS[selected];
    const url = buildUrl(endpoint, values);
    const curl = curlCommand(url, key);

    function selectEndpoint(index) {
      setSelected(index);
      setValues({});
      setResponse(null);
      setCopied(false);
    }

    function applyPreset(preset) {
      setSelected(preset.index);
      setValues(preset.values);
      setResponse(null);
    }

    function updateValue(name, value) {
      setValues(Object.assign({}, values, { [name]: value }));
    }

    async function sendRequest() {
      setLoading(true);
      setCopied(false);
      const started = performance.now();
      try {
        const result = await fetch(url, { headers: { "X-API-Key": key } });
        const text = await result.text();
        let body = text;
        try {
          body = JSON.parse(text);
        } catch (error) {
        }
        setResponse({ status: result.status, ms: Math.round(performance.now() - started), body: body });
      } catch (error) {
        setResponse({ status: "ERR", ms: Math.round(performance.now() - started), body: { error: String(error) } });
      } finally {
        setLoading(false);
      }
    }

    async function copyCurl() {
      try {
        await navigator.clipboard.writeText(curl);
        setCopied(true);
        setTimeout(function () { setCopied(false); }, 1600);
      } catch (error) {
        setCopied(false);
      }
    }

    const statusClass = response && typeof response.status === "number" && response.status < 400 ? "status ok" : "status bad";
    const responseText = response
      ? (typeof response.body === "string" ? response.body : JSON.stringify(response.body, null, 2))
      : "Send a request to see the JSON response here.";

    return h("div", { className: "app" },
      h("aside", { className: "sidebar" },
        h("div", { className: "brand" },
          h("div", { className: "logo" }, "M"),
          h("div", null, h("b", null, "Matchs Console"), h("span", null, "API v1 playground"))
        ),
        GROUPS.map(function (group) {
          return h("div", { key: group },
            h("div", { className: "side-title" }, group),
            ENDPOINTS.map(function (item, index) {
              if (item.group !== group) return null;
              return h("button", {
                key: item.path,
                className: "nav-item" + (selected === index ? " active" : ""),
                onClick: function () { selectEndpoint(index); }
              }, h("span", null, item.label), h("span", { className: "method" }, item.method));
            })
          );
        })
      ),
      h("main", { className: "main" },
        h("div", { className: "topbar" },
          h("div", null,
            h("h1", null, "API Playground"),
            h("div", { className: "sub" }, "Try every endpoint with the built-in demo key.")
          ),
          h("div", { className: "keybar" },
            h("a", { className: "btn btn-soft", href: "./client.html" }, "Client view"),
            h("span", { className: "badge" }, h("span", { className: "dot" }), "Unlimited demo quota"),
            h("input", {
              className: "key-input",
              type: showKey ? "text" : "password",
              value: key,
              "aria-label": "API key",
              onChange: function (event) { setKey(event.target.value); }
            }),
            h("button", { className: "btn", onClick: function () { setShowKey(!showKey); } }, showKey ? "Hide" : "Show")
          )
        ),
        h("div", { className: "stats" },
          h("div", { className: "stat" }, h("b", null, String(ENDPOINTS.length)), h("span", null, "endpoints")),
          h("div", { className: "stat" }, h("b", null, "3"), h("span", null, "competitions")),
          h("div", { className: "stat" }, h("b", null, "300/min"), h("span", null, "rate limit")),
          h("div", { className: "stat" }, h("b", null, "Max"), h("span", null, "monthly quota"))
        ),
        h("div", { className: "grid" },
          h("section", { className: "card" },
            h("h2", null, endpoint.label),
            h("p", { className: "hint" }, endpoint.method + " " + endpoint.path),
            h("div", { className: "presets" }, PRESETS.map(function (preset) {
              return h("button", { key: preset.label, className: "chip", onClick: function () { applyPreset(preset); } }, preset.label);
            })),
            endpoint.params.map(function (param) {
              return h(Field, { key: param.name, param: param, value: values[param.name], onChange: updateValue });
            }),
            h("div", { className: "urlbar" },
              h("div", { className: "url" }, url || BASE),
              h("button", { className: "btn btn-primary fixed", onClick: sendRequest, disabled: loading }, loading ? "Sending…" : "Send request")
            ),
            h("div", { className: "row" },
              h("button", { className: "btn btn-soft", onClick: copyCurl }, copied ? "Copied" : "Copy cURL"),
              h("button", { className: "btn", onClick: function () { setValues({}); setResponse(null); } }, "Clear")
            ),
            h("label", null, "cURL"),
            h("pre", null, curl)
          ),
          h("section", { className: "card" },
            h("div", { className: "response-head" },
              h("h2", null, "Response"),
              response ? h("span", { className: statusClass }, response.status + " · " + response.ms + "ms") : h("span", { className: "status" }, "idle")
            ),
            h("pre", null, responseText)
          )
        ),
        h("div", { className: "footer" }, "Matchs API · official-source Algerian football data · demo key is for testing only")
      )
    );
  }

  ReactDOM.createRoot(document.getElementById("root")).render(h(App));
})();
