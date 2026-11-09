// clip-ferry 운영자 페이지 (F38). 서버 값은 모두 textContent로만 넣는다.
"use strict";

(function () {
  const KEY = "cf-admin";
  let token = null;
  let email = "";
  let query = "";
  let offset = 0;
  let total = 0;
  let limit = 50;
  let searchTimer = null;

  const $ = (id) => document.getElementById(id);

  function load() {
    try {
      const v = JSON.parse(sessionStorage.getItem(KEY) || "null");
      if (v && v.token && v.exp > Date.now()) {
        token = v.token;
        email = v.email || "";
      }
    } catch (_) { /* 저장소를 못 쓰면 다시 로그인 */ }
  }

  function save(exp) {
    try { sessionStorage.setItem(KEY, JSON.stringify({ token, email, exp })); } catch (_) {}
  }

  function forget() {
    token = null;
    email = "";
    try { sessionStorage.removeItem(KEY); } catch (_) {}
  }

  // ------------------------------------------------------------ 표시 도우미

  const nf = new Intl.NumberFormat("ko-KR");
  const num = (n) => nf.format(n || 0);

  function bytes(n) {
    n = n || 0;
    const units = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return (i === 0 ? n : n.toFixed(n < 10 ? 1 : 0)) + " " + units[i];
  }

  const dayStr = (day) => (day == null ? "–" : new Date(day * 86400000).toISOString().slice(0, 10));

  function timeStr(ms) {
    if (!ms) return "–";
    const d = new Date(ms);
    return d.toLocaleString("ko-KR", { dateStyle: "short", timeStyle: "short" });
  }

  function cell(text, cls) {
    const td = document.createElement("td");
    td.textContent = text;
    if (cls) td.className = cls;
    return td;
  }

  function row(cells, onClick) {
    const tr = document.createElement("tr");
    cells.forEach((c) => tr.appendChild(c));
    if (onClick) {
      tr.className = "click";
      tr.tabIndex = 0;
      tr.addEventListener("click", onClick);
      tr.addEventListener("keydown", (e) => { if (e.key === "Enter") onClick(); });
    }
    return tr;
  }

  function emptyRow(tbody, cols, text) {
    const td = cell(text, "muted");
    td.colSpan = cols;
    tbody.replaceChildren(row([td]));
  }

  // ------------------------------------------------------------ 서버

  async function api(path) {
    const r = await fetch(path, { headers: { Authorization: "Bearer " + token } });
    if (r.status === 401) {
      forget();
      show();
      $("login-error").textContent = "로그인이 만료되었습니다. 다시 로그인하세요.";
      throw new Error("unauthorized");
    }
    if (r.status === 403) {
      forget();
      show();
      $("login-error").textContent = "허용 목록에 없는 계정입니다.";
      throw new Error("forbidden");
    }
    if (!r.ok) throw new Error("HTTP " + r.status);
    return r.json();
  }

  // 구글 로그인 버튼이 부른다 (data-callback)
  window.cfAdminGoogle = async function (resp) {
    $("login-error").textContent = "";
    try {
      const r = await fetch("/admin/api/session", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id_token: resp.credential }),
      });
      if (r.status === 403) {
        $("login-error").textContent = "허용 목록에 없는 계정입니다.";
        return;
      }
      if (!r.ok) {
        $("login-error").textContent = "로그인하지 못했습니다 (HTTP " + r.status + ").";
        return;
      }
      const v = await r.json();
      token = v.token;
      email = v.email;
      save(Date.now() + (v.expires_in - 60) * 1000);
      show();
    } catch (e) {
      $("login-error").textContent = "서버에 연결하지 못했습니다.";
    }
  };

  // ------------------------------------------------------------ 화면

  function show() {
    const inside = !!token;
    $("login").classList.toggle("hidden", inside);
    $("app").classList.toggle("hidden", !inside);
    $("logout").classList.toggle("hidden", !inside);
    $("who").textContent = inside ? email : "";
    if (inside) {
      refreshStats();
      refreshUsers();
    }
  }

  async function refreshStats() {
    $("app-error").textContent = "";
    let v;
    try { v = await api("/admin/api/stats?days=30"); } catch (e) { return fail(e); }
    const o = v.overview;
    $("t-users").textContent = num(o.users);
    $("t-dau").textContent = num(o.dau);
    $("t-mau").textContent = num(o.mau);
    const msgs = v.daily.reduce((a, d) => a + d.messages, 0);
    const by = v.daily.reduce((a, d) => a + d.bytes, 0);
    $("t-msgs").textContent = num(msgs);
    $("t-bytes").textContent = bytes(by);
    $("updated").textContent = "· " + new Date().toLocaleTimeString("ko-KR") + " 기준";

    const pb = $("platforms");
    if (o.platforms.length === 0) emptyRow(pb, 3, "등록된 기기가 없습니다");
    else pb.replaceChildren(...o.platforms.map((p) =>
      row([cell(p.platform), cell(num(p.devices), "num"), cell(num(p.seen_30d), "num")])));

    const max = Math.max(1, ...v.daily.map((d) => d.messages));
    const db = $("daily");
    db.replaceChildren(...v.daily.slice().reverse().map((d) => {
      const barCell = document.createElement("td");
      barCell.className = "bar-cell";
      const bar = document.createElement("div");
      bar.className = "bar";
      bar.style.width = (d.messages ? Math.max(1, (d.messages / max) * 100) : 0) + "%";
      barCell.appendChild(bar);
      return row([
        cell(dayStr(d.day)), cell(num(d.active_users), "num"), cell(num(d.new_users), "num"),
        cell(num(d.messages), "num"), cell(bytes(d.bytes), "num"), barCell,
      ]);
    }));
  }

  async function refreshUsers() {
    let v;
    const qs = "?q=" + encodeURIComponent(query) + "&offset=" + offset;
    try { v = await api("/admin/api/users" + qs); } catch (e) { return fail(e); }
    total = v.total;
    limit = v.limit;
    const ub = $("users");
    if (v.users.length === 0) emptyRow(ub, 8, query ? "찾는 사용자가 없습니다" : "사용자가 없습니다");
    else ub.replaceChildren(...v.users.map((u) => row([
      cell(u.email), cell(u.name), cell(u.provider), cell(timeStr(u.created_at_ms)),
      cell(num(u.device_count), "num"), cell(num(u.messages_30d), "num"),
      cell(bytes(u.bytes_30d), "num"), cell(dayStr(u.last_active_day)),
    ], () => openUser(u.id))));
    const end = Math.min(total, offset + v.users.length);
    $("users-count").textContent = total ? (offset + 1) + "–" + end + " / " + num(total) + "명" : "";
    $("prev").disabled = offset === 0;
    $("next").disabled = end >= total;
  }

  async function openUser(id) {
    let v;
    try { v = await api("/admin/api/users/" + encodeURIComponent(id)); } catch (e) { return fail(e); }
    const u = v.user;
    $("d-title").textContent = u.email;
    const info = [
      ["이름", u.name || "–"], ["로그인", u.provider], ["계정 ID", u.id],
      ["가입", timeStr(u.created_at_ms)], ["마지막 로그인", timeStr(u.last_login_ms)],
    ];
    $("d-info").replaceChildren(...info.flatMap(([k, val]) => {
      const dt = document.createElement("dt");
      dt.textContent = k;
      const dd = document.createElement("dd");
      dd.textContent = val;
      return [dt, dd];
    }));
    const devs = $("d-devices");
    if (v.devices.length === 0) emptyRow(devs, 4, "기기가 없습니다");
    else devs.replaceChildren(...v.devices.map((d) => row([
      cell(d.name), cell(d.platform), cell(timeStr(d.created_at_ms)), cell(timeStr(d.last_seen_ms)),
    ])));
    const us = $("d-usage");
    if (v.usage.length === 0) emptyRow(us, 3, "최근 90일 사용 기록이 없습니다");
    else us.replaceChildren(...v.usage.slice().reverse().map((d) => row([
      cell(dayStr(d.day)), cell(num(d.messages), "num"), cell(bytes(d.bytes), "num"),
    ])));
    $("detail").classList.remove("hidden");
    $("detail").scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function fail(e) {
    if (e && (e.message === "unauthorized" || e.message === "forbidden")) return;
    $("app-error").textContent = "불러오지 못했습니다: " + (e && e.message ? e.message : e);
  }

  document.addEventListener("DOMContentLoaded", () => {
    $("logout").addEventListener("click", () => {
      forget();
      try { if (window.google) google.accounts.id.disableAutoSelect(); } catch (_) {}
      show();
    });
    $("q").addEventListener("input", (e) => {
      clearTimeout(searchTimer);
      searchTimer = setTimeout(() => {
        query = e.target.value.trim();
        offset = 0;
        refreshUsers();
      }, 300);
    });
    $("prev").addEventListener("click", () => { offset = Math.max(0, offset - limit); refreshUsers(); });
    $("next").addEventListener("click", () => { offset += limit; refreshUsers(); });
    $("d-close").addEventListener("click", () => $("detail").classList.add("hidden"));
    load();
    show();
  });
})();
