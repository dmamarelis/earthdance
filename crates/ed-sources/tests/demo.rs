use ed_sources::{data_root, demo};

#[test]
fn demo_set_loads_completely() {
    let t = std::time::Instant::now();
    let (ds, wx) = demo::load(&data_root()).expect("demo set");
    println!("loaded in {:?}", t.elapsed());
    assert!(ds.demo);
    assert_eq!(ds.layers.len(), 6);
    for l in &ds.layers {
        assert_eq!(l.frames.len(), 10, "{}", l.info.id);
        for f in &l.frames {
            let v = f.values.as_ref().expect("values");
            let img = f.image.as_ref().expect("image");
            assert_eq!((v.w, v.h), (160, 95));
            assert_eq!((img.w, img.h), (640, 380));
        }
    }
    assert_eq!(ds.optical.len(), 10);
    assert_eq!(ds.points.len(), 11);
    assert_eq!(ds.regions.regions.len(), 5);
    assert!(ds.validation.is_some());
    assert_eq!(ds.content.story.len(), 6);
    assert_eq!(wx.unwrap().by_region.len(), 5);
    // hover value at the centre of the site
    let hh = &ds.layer("hh").unwrap().frames[0];
    let v = hh.values.as_ref().unwrap().sample(ds.site.center[0], ds.site.center[1]);
    println!("hh at centre: {v:?}");
}
