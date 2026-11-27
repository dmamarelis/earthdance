//! The Status view's computations (the non-DOM half of `js/status.js`): every area assessed, the lake's
//! forecast brief, and the "what if" run through the fitted model.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ask::{self, Brief};
use crate::forecast::{self, Options, Run};
use crate::insights::{assess, scenario, Assessment, Level, Scenario, WhatIf};
use crate::model::{Dataset, ForecastModel, Region, Regions, WeatherSet};
use crate::Date;

/// Today's picture of every area.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub today: Date,
    /// By area id.
    pub assessments: BTreeMap<String, Assessment>,
    /// The lake forecast (only with a fitted model and once the weather is known).
    pub brief: Option<Brief>,
}

impl Status {
    pub fn get(&self, id: &str) -> &Assessment {
        &self.assessments[id]
    }

    /// Areas, most severe first (stable in the original order otherwise).
    pub fn sorted<'a>(&self, regions: &'a Regions) -> Vec<&'a Region> {
        let mut v: Vec<&Region> = regions.regions.iter().collect();
        v.sort_by_key(|r| std::cmp::Reverse(self.get(&r.id).status));
        v
    }

    pub fn counts(&self) -> [usize; 3] {
        let mut c = [0; 3];
        for a in self.assessments.values() {
            c[a.status as usize] += 1;
        }
        c
    }

    pub fn lake_id(&self, fc: Option<&ForecastModel>) -> Option<String> {
        self.brief.as_ref().and(fc).map(|f| f.region.clone())
    }
}

/// Runs every rule on every area. `weather_loaded = false` while the weather is still being fetched:
/// the forecast waits for it (the legacy `wxState !== 'loading'`).
pub fn compute(
    regions: &Regions,
    wx: Option<&WeatherSet>,
    weather_loaded: bool,
    ref_name: &str,
    fc: Option<&ForecastModel>,
    today: Date,
) -> Status {
    let sm: Vec<f64> = regions.regions.iter().filter_map(|r| r.series.soil_moist_pct.as_ref().and_then(|v| v.last().copied())).collect();
    let soil_ref = (sm.len() > 1).then(|| sm.iter().sum::<f64>() / sm.len() as f64);
    let wx_of = |id: &str| wx.and_then(|w| w.by_region.get(id));
    let mut assessments: BTreeMap<String, Assessment> = regions
        .regions
        .iter()
        .map(|r| (r.id.clone(), assess(r, wx_of(&r.id), &regions.thresholds, today, ref_name, soil_ref)))
        .collect();

    // with a fitted model, the lake finding comes from the forecast instead of the straight-line projection
    let lake = fc.filter(|_| weather_loaded).and_then(|fc| regions.regions.iter().find(|r| r.id == fc.region && r.reservoir.is_some()));
    let brief = lake.map(|lake| {
        let fc = fc.expect("lake implies a model");
        ask::brief(fc, lake.reservoir.as_ref().expect("checked"), wx_of(&lake.id), today, &lake.name)
    });
    if let (Some(b), Some(lake)) = (&brief, lake) {
        let a = assessments.get_mut(&lake.id).expect("assessed");
        let f = ask::finding(b, &regions.thresholds);
        match a.findings.iter().position(|x| x.key == "reservoir") {
            Some(i) => a.findings[i] = f.clone(),
            None => a.findings.push(f.clone()),
        }
        if let Some(r) = a.reservoir.as_mut() {
            r.level = f.level;
            r.finding = f;
        }
        a.resort();
    }
    Status { today, assessments, brief }
}

/// The area the what-if applies to: the first with a measured reservoir.
pub fn lake_region(regions: &Regions) -> Option<&Region> {
    regions.regions.iter().find(|r| r.reservoir.as_ref().is_some_and(|x| !x.volume.is_empty()))
}

/// A what-if result, either from the fitted model or from the simple water balance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum WhatIfResult {
    Model(ModelScenario),
    Simple(Scenario),
}

impl WhatIfResult {
    pub fn low(&self) -> f64 {
        match self {
            Self::Model(m) => m.low,
            Self::Simple(s) => s.low,
        }
    }
    pub fn limit(&self) -> f64 {
        match self {
            Self::Model(m) => m.limit,
            Self::Simple(s) => s.limit,
        }
    }
    pub fn unit(&self) -> &str {
        match self {
            Self::Model(_) => "km²",
            Self::Simple(s) => &s.unit,
        }
    }
    pub fn days_below(&self) -> f64 {
        match self {
            Self::Model(m) => m.days_below as f64,
            Self::Simple(s) => s.days_below,
        }
    }
    pub fn scale(&self) -> f64 {
        match self {
            Self::Model(m) => m.scale,
            Self::Simple(s) => s.scale,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelScenario {
    pub run: Run,
    /// Lake area on 1 June next year (or the lowest of next summer).
    pub low: f64,
    pub limit: f64,
    pub area_now: f64,
    pub days_below: usize,
    pub next: Option<Date>,
    /// Linear scale of today's shoreline for the map.
    pub scale: f64,
}

/// The what-if run through the fitted model (`modelScenario()` in status.js).
pub fn model_scenario(fc: &ForecastModel, region: &Region, wx: Option<&WeatherSet>, today: Date, w: WhatIf) -> Option<ModelScenario> {
    let res = region.reservoir.as_ref()?;
    let run = forecast::run(fc, res, wx.and_then(|x| x.by_region.get(&region.id)), today, Options::what_if(w.rain, w.irrigation));
    let june = run.dates.iter().position(|&d| d > today && d.md() == (6, 1));
    let low = june.map(|i| run.p50[i]).or(run.low_next).unwrap_or(0.0);
    let area_now = *res.area_km2.as_ref()?.last()?;
    Some(ModelScenario {
        low,
        limit: res.limit,
        area_now,
        days_below: run.days_below_next,
        next: run.next.map(|n| n.mid),
        scale: (low.max(0.3) / area_now).sqrt(),
        run,
    })
}

/// The what-if for the data set's lake, choosing the model when there is one.
pub fn what_if(ds: &Dataset, status: &Status, wx: Option<&WeatherSet>, w: WhatIf) -> Option<(String, WhatIfResult)> {
    let r = lake_region(&ds.regions)?;
    let with_model = status.brief.is_some() && ds.forecast.as_ref().is_some_and(|f| f.region == r.id);
    let res = if with_model {
        WhatIfResult::Model(model_scenario(ds.forecast.as_ref()?, r, wx, status.today, w)?)
    } else {
        WhatIfResult::Simple(scenario(r, status.assessments.get(&r.id)?, w)?)
    };
    Some((r.id.clone(), res))
}

/// The level of the area that drives the lake alert.
pub fn lake_level(status: &Status, fc: Option<&ForecastModel>) -> Option<Level> {
    let id = status.lake_id(fc)?;
    status.assessments.get(&id)?.reservoir.as_ref().map(|r| r.level)
}
