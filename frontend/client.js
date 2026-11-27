(function () {
  const h = React.createElement;
  const BASE = window.MATCHS_API_BASE || "";
  const KEY = window.MATCHS_DEMO_KEY || "";
  const TABS = [
    { id: "matches", label: "Matches" },
    { id: "standings", label: "Standings" },
    { id: "live", label: "Near live" },
    { id: "search", label: "Search" },
    { id: "status", label: "Sources" }
  ];

  function query(params) {
    const search = new URLSearchParams();
    Object.keys(params).forEach(function (name) {
      if (params[name]) search.set(name, params[name]);
    });
    const value = search.toString();
    return value ? "?" + value : "";
  }

  async function api(path) {
    const response = await fetch(BASE + path, { headers: { "X-API-Key": KEY } });
    const text = await response.text();
    let body = text;
    try {
      body = JSON.parse(text);
    } catch (error) {
    }
    if (!response.ok) {
      throw new Error((body && body.message) || (body && body.error) || ("HTTP " + response.status));
    }
    return body;
  }

  function short(value) {
    return value ? String(value).slice(0, 3).toUpperCase() : "?";
  }

  function dateLabel(value) {
    if (!value) return "—";
    return new Date(value).toLocaleString(undefined, {
      month: "short", day: "numeric", hour: "2-digit", minute: "2-digit"
    });
  }

  function MatchCard(props) {
    const match = props.match;
    const live = match.is_live || match.status === "live";
    const statusClass = live ? "status live" : match.status === "finished" ? "status finished" : "status scheduled";
    const score = match.home_score === null || match.home_score === undefined
      ? "vs"
      : match.home_score + " – " + match.away_score;
    return h("article", { className: "match", key: match.id },
      h("div", { className: "match-head" },
        h("span", { className: "league" }, match.competition_code),
        h("span", null, "Round " + match.round + " · " + dateLabel(match.kickoff_utc))
      ),
      h("div", { className: "scoreline" },
        h("div", { className: "team" },
          h("div", { className: "badge" }, short(match.home.short_name || match.home.name)),
          h("div", null, h("b", null, match.home.short_name || match.home.name), h("span", null, match.home.name))
        ),
        h("div", { className: "score" }, score),
        h("div", { className: "team right" },
          h("div", null, h("b", null, match.away.short_name || match.away.name), h("span", null, match.away.name)),
          h("div", { className: "badge" }, short(match.away.short_name || match.away.name))
        )
      ),
      h("span", { className: statusClass }, live ? "Live " + (match.live_minute || "") : match.status),
      h("div", { className: "foot" },
        h("span", null, match.venue || match.source),
        match.source_url ? h("a", { href: match.source_url, target: "_blank", rel: "noreferrer" }, "Source") : null
      )
    );
  }

  function StandingsTable(props) {
    const rows = props.rows || [];
    return h("div", { className: "panel table-wrap" },
      h("div", { className: "section-title" }, h("h2", null, "League table"), h("span", null, rows.length + " teams")),
      h("table", null,
        h("thead", null, h("tr", null,
          ["#", "Club", "P", "W", "D", "L", "GF", "GA", "GD", "Pts"].map(function (label) {
            return h("th", { key: label }, label);
          })
        )),
        h("tbody", null, rows.map(function (row) {
          return h("tr", { key: row.club.id },
            h("td", null, h("span", { className: "pos" + (row.position <= 3 ? " top" : "") }, row.position)),
            h("td", null, row.club.name),
            h("td", null, row.played), h("td", null, row.wins), h("td", null, row.draws), h("td", null, row.losses),
            h("td", null, row.goals_for), h("td", null, row.goals_against), h("td", null, row.goal_difference),
            h("td", { className: "pts" }, row.points)
          );
        }))
      )
    );
  }

  function App() {
    const [tab, setTab] = React.useState("matches");
    const [competitions, setCompetitions] = React.useState([]);
    const [seasons, setSeasons] = React.useState([]);
    const [groups, setGroups] = React.useState([]);
    const [filters, setFilters] = React.useState({ competition: "", season: "", status: "", group: "", q: "" });
    const [data, setData] = React.useState(null);
    const [loading, setLoading] = React.useState(false);
    const [error, setError] = React.useState("");
    const [auto, setAuto] = React.useState(false);
    const [refresh, setRefresh] = React.useState(0);

    React.useEffect(function () {
      api("/api/v1/competitions").then(setCompetitions).catch(function () {});
      api("/api/v1/seasons").then(setSeasons).catch(function () {});
      api("/api/v1/groups").then(setGroups).catch(function () {});
    }, []);

    React.useEffect(function () {
      if (!seasons.length || filters.competition) return;
      const first = seasons[0];
      setFilters(function (current) { return Object.assign({}, current, { competition: first.competition_code, season: first.id }); });
    }, [seasons, filters.competition]);

    React.useEffect(function () {
      let active = true;
      setLoading(true);
      setError("");
      setData(null);

      let path = "";
      if (tab === "matches") {
        path = "/api/v1/matches" + query({ competition: filters.competition, season: filters.season, status: filters.status, per_page: "24" });
      } else if (tab === "standings") {
        if (!filters.season) {
          setLoading(false);
          return function () { active = false; };
        }
        path = "/api/v1/standings" + query({ season: filters.season, group: filters.group, competition: filters.competition });
      } else if (tab === "live") {
        path = "/api/v1/live";
      } else if (tab === "search") {
        if (!filters.q.trim()) {
          setLoading(false);
          return function () { active = false; };
        }
        path = "/api/v1/search" + query({ q: filters.q.trim() });
      } else {
        path = "/api/v1/status";
      }

      api(path).then(function (body) {
        if (!active) return;
        let result = body;
        if (tab === "matches" && filters.q.trim()) {
          const term = filters.q.trim().toLowerCase();
          result = body.filter(function (match) {
            return (match.home.name || "").toLowerCase().includes(term) || (match.away.name || "").toLowerCase().includes(term);
          });
        }
        setData(result);
      }).catch(function (err) {
        if (active) setError(err.message || "Request failed");
      }).finally(function () {
        if (active) setLoading(false);
      });

      return function () { active = false; };
    }, [tab, filters.competition, filters.season, filters.status, filters.group, filters.q, refresh]);

    React.useEffect(function () {
      if (!auto || tab !== "live") return;
      const timer = setInterval(function () { setRefresh(function (value) { return value + 1; }); }, 60000);
      return function () { clearInterval(timer); };
    }, [auto, tab]);

    function update(name, value) {
      setFilters(function (current) { return Object.assign({}, current, { [name]: value }); });
    }

    const seasonOptions = seasons.filter(function (season) {
      return !filters.competition || season.competition_code === filters.competition;
    });
    const groupOptions = groups.filter(function (group) {
      return !filters.competition || group.competition_code === filters.competition;
    });
    const matchData = Array.isArray(data) ? data : [];
    const searchClubs = data && data.clubs ? data.clubs : [];
    const searchMatches = data && data.matches ? data.matches : [];

    return h("div", { className: "shell" },
      h("header", { className: "top" },
        h("div", { className: "brand" },
          h("div", { className: "logo" }, "M"),
          h("div", null, h("b", null, "Matchs Client"), h("span", null, "Algerian football data"))
        ),
        h("div", { className: "links" },
          h("a", { className: "link", href: "./" }, "API playground"),
          h("a", { className: "link", href: "/docs" }, "Docs"),
          h("a", { className: "link", href: "/request-access" }, "Get API key")
        )
      ),
      h("section", { className: "hero" },
        h("span", { className: "pill" }, h("span", { className: "dot" }), " Public data view"),
        h("h1", null, "Fixtures, tables and near-live scores."),
        h("p", null, "A simple client built on the Matchs API. Browse Ligue 1, Ligue 2 and Inter-régions without writing code.")
      ),
      h("div", { className: "tabs" }, TABS.map(function (item) {
        return h("button", {
          key: item.id,
          className: "tab" + (tab === item.id ? " active" : ""),
          onClick: function () { setTab(item.id); }
        }, item.label);
      })),
      (tab === "matches" || tab === "standings") ? h("div", { className: "filters" },
        h("div", { className: "filter" }, h("label", null, "Competition"), h("select", {
          value: filters.competition,
          onChange: function (event) {
            setFilters(function (current) {
              const competition = event.target.value;
              const season = seasons.find(function (item) { return item.competition_code === competition; });
              return Object.assign({}, current, { competition: competition, season: season ? season.id : "", group: "" });
            });
          }
        }, h("option", { value: "" }, "All competitions"), competitions.map(function (item) {
          return h("option", { key: item.code, value: item.code }, item.name);
        }))),
        h("div", { className: "filter" }, h("label", null, "Season"), h("select", {
          value: filters.season,
          onChange: function (event) { update("season", event.target.value); }
        }, h("option", { value: "" }, "Choose season"), seasonOptions.map(function (item) {
          return h("option", { key: item.id, value: item.id }, item.label);
        }))),
        tab === "standings" ? h("div", { className: "filter" }, h("label", null, "Group"), h("select", {
          value: filters.group,
          onChange: function (event) { update("group", event.target.value); }
        }, h("option", { value: "" }, "All groups"), groupOptions.map(function (item) {
          return h("option", { key: item.id, value: item.id }, item.name);
        }))) : null,
        tab === "matches" ? h("div", { className: "filter" }, h("label", null, "Status"), h("select", {
          value: filters.status,
          onChange: function (event) { update("status", event.target.value); }
        }, ["", "scheduled", "live", "finished"].map(function (value) {
          return h("option", { key: value, value: value }, value || "All statuses");
        }))) : null,
        tab === "matches" ? h("div", { className: "filter" }, h("label", null, "Filter teams"), h("input", {
          value: filters.q,
          placeholder: "Club name",
          onChange: function (event) { update("q", event.target.value); }
        })) : null,
        h("button", { className: "btn", onClick: function () { setRefresh(function (value) { return value + 1; }); } }, "Refresh")
      ) : null,
      tab === "search" ? h("div", { className: "filters" },
        h("div", { className: "filter" }, h("label", null, "Search clubs and matches"), h("input", {
          value: filters.q,
          placeholder: "Try JS Kabylie or MC Alger",
          onChange: function (event) { update("q", event.target.value); }
        })),
        h("button", { className: "btn btn-primary", onClick: function () { setRefresh(function (value) { return value + 1; }); } }, "Search")
      ) : null,
      tab === "live" ? h("div", { className: "filters" },
        h("span", { className: "pill" }, h("span", { className: "dot" }), auto ? "Auto-refresh every 60s" : "Manual refresh"),
        h("button", { className: "btn", onClick: function () { setAuto(!auto); } }, auto ? "Stop auto-refresh" : "Start auto-refresh"),
        h("button", { className: "btn btn-primary", onClick: function () { setRefresh(function (value) { return value + 1; }); } }, "Refresh now")
      ) : null,
      error ? h("div", { className: "state error" }, error) : null,
      loading ? h("div", { className: "state" }, "Loading data…") : null,
      !loading && !error && tab === "matches" ? (matchData.length
        ? h("div", { className: "grid" }, matchData.map(function (match) { return h(MatchCard, { key: match.id, match: match }); }))
        : h("div", { className: "state" }, "No matches for these filters.")) : null,
      !loading && !error && tab === "standings" ? (filters.season
        ? h(StandingsTable, { rows: matchData })
        : h("div", { className: "state" }, "Choose a season to see the table.")) : null,
      !loading && !error && tab === "live" ? (matchData.length
        ? h("div", { className: "grid" }, matchData.map(function (match) { return h(MatchCard, { key: match.id, match: match }); }))
        : h("div", { className: "state" }, "No matches are marked live right now.")) : null,
      !loading && !error && tab === "search" ? (filters.q.trim()
        ? h("div", null,
            h("div", { className: "section-title" }, h("h2", null, "Clubs"), h("span", null, searchClubs.length + " found")),
            searchClubs.length ? h("div", { className: "cards" }, searchClubs.map(function (club) {
              return h("div", { className: "metric", key: club.id }, h("b", null, club.name), h("span", null, club.short_name));
            })) : h("div", { className: "state" }, "No clubs found."),
            h("div", { className: "section-title" }, h("h2", null, "Matches"), h("span", null, searchMatches.length + " found")),
            searchMatches.length ? h("div", { className: "grid" }, searchMatches.map(function (match) {
              return h(MatchCard, { key: match.id, match: match });
            })) : h("div", { className: "state" }, "No matches found.")
          )
        : h("div", { className: "state" }, "Type a club or team name to search.")) : null,
      !loading && !error && tab === "status" ? (matchData.length
        ? h("div", { className: "cards" }, matchData.map(function (row) {
            return h("div", { className: "metric", key: row.source },
              h("b", null, row.source.toUpperCase()),
              h("span", null, row.matches + " matches · " + row.live + " live"),
              h("span", null, "Latest: " + dateLabel(row.last_match))
            );
          }))
        : h("div", { className: "state" }, "No source data yet.")) : null,
      h("div", { className: "footer" }, "Powered by the Matchs API · demo key embedded for testing")
    );
  }

  ReactDOM.createRoot(document.getElementById("root")).render(h(App));
})();
