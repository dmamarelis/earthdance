//! The regional brief as a printable, self-contained HTML file (the legacy `exportReport()`), opened in the
//! system browser to print or save as PDF. Which report format the native app should finally produce is
//! decision D4; this keeps the feature working meanwhile.

use std::fmt::Write as _;
use std::path::PathBuf;

use ed_core::ask;
use ed_core::insights::WhatIf;
use ed_core::model::{Dataset, WeatherSet};
use ed_core::status::{Status, WhatIfResult};
use ed_core::text::{fd, fill, fmt_date, js_number, signed, t, tf, to_fixed, DateStyle, Lang};
use ed_core::{Date, Level};

use crate::app::WxState;
use crate::panels::strip_tags;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn color(l: Level) -> &'static str {
    match l {
        Level::Alert => "#d9443a",
        Level::Watch => "#c98a10",
        Level::Ok => "#3f9a55",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn export(ds: &Dataset, st: &Status, wx: Option<&WeatherSet>, wx_state: WxState, today: Date, l: Lang, w: WhatIf, scn: Option<WhatIfResult>) -> std::io::Result<PathBuf> {
    let refn = ds.reference_name();
    let last = ds.regions.regions.iter().filter_map(|r| r.series.dates.last()).max().copied().unwrap_or(today);
    let wxl = if wx_state == WxState::Snapshot { tf("brief.wx.snapshot", l, &[("d", &fd(wx.and_then(|w| w.saved), l))]) } else { t(&format!("brief.wx.{}", wx_state.key()), l).to_string() };
    let mut h = String::new();
    let _ = write!(
        h,
        "<!doctype html><html lang=\"{}\"><meta charset=\"utf-8\"><title>EarthDance · {}</title><style>
body{{font:14px/1.45 system-ui,sans-serif;color:#111;max-width:860px;margin:32px auto;padding:0 20px}}
header{{display:flex;justify-content:space-between;align-items:flex-end;border-bottom:2px solid #111;padding-bottom:8px}}
h1{{margin:4px 0 0;font-size:24px}} h2{{margin-top:26px;font-size:17px;border-bottom:1px solid #ccc}} small{{color:#666}}
table{{border-collapse:collapse;width:100%}} td,th{{border-bottom:1px solid #ddd;padding:5px 6px;text-align:left}}
.dot{{display:inline-block;width:9px;height:9px;border-radius:50%;margin-right:6px}} .item{{border-left:4px solid;padding:4px 12px;margin:10px 0}}
.basis{{color:#666;font-size:12px}} .demo{{background:#fff4d6;padding:8px;border-radius:6px}} footer{{margin-top:30px;color:#666;font-size:12px}}
</style><body>",
        l.code(),
        esc(t("report.title", l))
    );
    let _ = write!(
        h,
        "<header><div><small>EarthDance · {}</small><h1>{} — {}</h1></div><div>{}</div></header>",
        esc(t("report.kicker", l)),
        esc(ds.site.name.get(l)),
        esc(t("report.title", l)),
        fmt_date(today, l, DateStyle::WeekdayDayMonthYear)
    );
    if ds.demo {
        let _ = write!(h, "<p class=demo>{}</p>", esc(t("report.demo", l)));
    }
    let radar = tf("brief.radarText", l, &[("d", &fd(Some(last), l)), ("n", &today.since(last).to_string())]);
    let _ = write!(h, "<p><small>{}: {} · {}: {}</small></p>", esc(t("brief.radar", l)), strip_tags(&radar), esc(t("brief.wx", l)), strip_tags(&wxl));
    let _ = write!(
        h,
        "<h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{} ({})</th><th>Δ {refn}</th><th>{}</th><th>{} (mm)</th></tr>",
        esc(t("report.summary", l)),
        esc(t("report.area", l)),
        esc(t("report.status", l)),
        esc(t("rg.tile.rate", l)),
        esc(t("unit.mmyr", l)),
        esc(t("rg.tile.soil", l)),
        esc(t("rg.tile.rain", l))
    );
    for r in st.sorted(&ds.regions) {
        let a = st.get(&r.id);
        let _ = write!(
            h,
            "<tr><td><span class=dot style=\"background:{}\"></span>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            color(a.status),
            esc(r.name.get(l)),
            esc(t(&format!("st.{}", a.status.key()), l)),
            a.ground.rate.map_or("–".into(), |v| signed(v, 0)),
            a.reference.as_ref().map_or("–".into(), |x| format!("{} {}", signed(x.diff, 1), if x.agrees { "✓" } else { "✗" })),
            a.soil.as_ref().map_or("–".into(), |s| format!("{}%", to_fixed(s.now, 0))),
            a.weather.as_ref().map_or("–".into(), |w| to_fixed(w.rain30, 0)),
        );
    }
    h.push_str("</table>");
    let _ = write!(h, "<h2>{}</h2>", esc(t("report.actions", l)));
    let mut any = false;
    for r in st.sorted(&ds.regions) {
        for f in st.get(&r.id).findings.iter().filter(|f| f.level != Level::Ok) {
            any = true;
            let _ = write!(h, "<div class=item style=\"border-color:{}\"><h3>{} · {}</h3><p>{}</p>", color(f.level), esc(r.name.get(l)), esc(f.title.get(l)), esc(f.text.get(l)));
            if let Some(act) = &f.action {
                let _ = write!(h, "<p><b>{}:</b> {}</p>", esc(t("action.title", l)), esc(act.get(l)));
            }
            let _ = write!(h, "<p class=basis>{}</p></div>", esc(f.basis.get(l)));
        }
    }
    if !any {
        let _ = write!(h, "<p>{}</p>", esc(t("brief.none", l)));
    }
    if let Some(b) = &st.brief {
        let _ = write!(h, "<h2>{}</h2>", esc(t("report.forecast", l)));
        for (k, line) in ["al3.seen", "al3.coming", "al3.do"].iter().zip(ask::lines(b, l)) {
            let _ = write!(h, "<p><b>{}:</b> {}</p>", esc(t(k, l)), esc(&line));
        }
        let _ = write!(h, "<p class=basis>{}</p>", esc(&ask::backtest_line(b, l)));
    }
    if let Some(s) = scn.filter(|_| w.is_change()) {
        let (key, next) = match &s {
            WhatIfResult::Model(m) => ("report.scn.model", m.next.map_or("–".into(), |d| fd(Some(d), l))),
            WhatIfResult::Simple(_) => ("report.scn", "–".into()),
        };
        let txt = fill(
            t(key, l),
            &[
                ("rain", &js_number(w.rain)),
                ("irr", &js_number(w.irrigation)),
                ("low", &to_fixed(s.low(), 0)),
                ("unit", s.unit()),
                ("limit", &js_number(s.limit())),
                ("days", &js_number(s.days_below())),
                ("next", &next),
            ],
        );
        let _ = write!(h, "<h2>{}</h2><p>{}</p>", esc(t("scn.title", l)), strip_tags(&txt));
    }
    let _ = write!(h, "<footer>{}</footer><script>print()</script></body></html>", esc(t("report.foot", l)));
    let dir = ed_sources::cache::dir().join("reports");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("earthdance-{}-{today}.html", l.code()));
    std::fs::write(&path, h)?;
    Ok(path)
}
