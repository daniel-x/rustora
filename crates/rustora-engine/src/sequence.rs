// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `SequenceSet`, `ClassicTilesetSpecificSpriteSequence` data and `Graphics/Animation.cs`, without sprites.
//! Sequence lengths with `Length: *` are resolved lazily from the sprite file header.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::fieldloader::get_value;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::WAngle;

pub type ReadFile = Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

fn node<'a>(data: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
    data.nodes.iter().find(|n| n.key == key)
}

fn node_d<'a>(data: &'a MiniYaml, defaults: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
    node(data, key).or_else(|| node(defaults, key))
}

fn field_i32(data: &MiniYaml, defaults: &MiniYaml, key: &str, fallback: i32) -> i32 {
    node_d(data, defaults, key).map_or(fallback, |n| get_value::<i32>(key, n.value.value.as_deref()).unwrap_or(fallback))
}

fn field_bool(data: &MiniYaml, defaults: &MiniYaml, key: &str) -> bool {
    node_d(data, defaults, key).is_some_and(|n| n.value.value_str().eq_ignore_ascii_case("true"))
}

/// A sprite sequence: the data the simulation reads, plus the yaml for the renderer.
pub struct Sequence {
    pub image: String,
    pub name: String,
    pub start: i32,
    declared_length: Option<i32>,
    pub stride: Option<i32>,
    pub facings: i32,
    pub reverse_facings: bool,
    pub interpolated_facings: Option<i32>,
    pub tick: i32,
    pub reverses: bool,
    pub data: MiniYaml,
    pub defaults: MiniYaml,
    tileset: String,
    read_file: ReadFile,
    resolved_length: OnceCell<i32>,
}

impl Sequence {
    fn new(image: &str, name: &str, data: &MiniYaml, defaults: &MiniYaml, tileset: &str, read_file: ReadFile) -> Sequence {
        let is_star = node_d(data, defaults, "Length").is_some_and(|n| n.value.value_str() == "*");
        let declared_length = if is_star {
            None
        } else {
            Some(field_i32(data, defaults, "Length", 1))
        };
        let mut facings = field_i32(data, defaults, "Facings", 1);
        let reverse_facings = facings < 0;
        if reverse_facings {
            facings = -facings;
        }
        Sequence {
            image: image.to_string(),
            name: name.to_string(),
            start: field_i32(data, defaults, "Start", 0),
            declared_length,
            stride: node_d(data, defaults, "Stride").map(|n| get_value::<i32>("Stride", n.value.value.as_deref()).expect("Stride")),
            facings,
            reverse_facings,
            interpolated_facings: node_d(data, defaults, "InterpolatedFacings")
                .map(|n| get_value::<i32>("InterpolatedFacings", n.value.value.as_deref()).expect("InterpolatedFacings")),
            tick: field_i32(data, defaults, "Tick", 40),
            reverses: field_bool(data, defaults, "Reverses"),
            data: data.clone(),
            defaults: defaults.clone(),
            tileset: tileset.to_string(),
            read_file,
            resolved_length: OnceCell::new(),
        }
    }

    /// `ISpriteSequence.Facings`.
    /// `ISpriteSequence.IgnoreWorldTint`: the sprites are not lit by `ITerrainLighting`.
    pub fn ignore_world_tint(&self) -> bool {
        field_bool(&self.data, &self.defaults, "IgnoreWorldTint")
    }

    pub fn facings_or_interpolated(&self) -> i32 {
        self.interpolated_facings.unwrap_or(self.facings)
    }

    fn filename_for_tileset(&self, data: &MiniYaml) -> Option<String> {
        let t = node(data, "TilesetFilenames").or_else(|| node(&self.defaults, "TilesetFilenames"))?;
        node(&t.value, &self.tileset).map(|n| n.value.value_str().to_string())
    }

    fn file_frames(&self, filename: &str) -> i32 {
        let bytes = (self.read_file)(filename).unwrap_or_else(|| panic!("sprite file {filename} not found ({}.{})", self.image, self.name));
        rustora_formats::sprite::frame_count(&bytes).unwrap_or_else(|| panic!("unknown sprite format of {filename}")) as i32
    }

    /// Length of `allSprites` in `DefaultSpriteSequence.ResolveSprites`.
    fn all_sprites_count(&self) -> i32 {
        if let Some(c) = node(&self.data, "Combine") {
            let mut total = 0;
            for sub in &c.value.nodes {
                let d = &sub.value;
                if let Some(f) = node(d, "Frames") {
                    total += get_value::<Vec<i32>>("Frames", f.value.value.as_deref()).expect("Frames").len() as i32;
                } else if node(d, "Length").map(|n| n.value.value_str()) != Some("*") {
                    total += node(d, "Length").map_or(1, |n| get_value::<i32>("Length", n.value.value.as_deref()).expect("Length"));
                } else {
                    let name = self
                        .filename_for_tileset(d)
                        .or_else(|| node(d, "Filename").map(|n| n.value.value_str().to_string()))
                        .expect("combine filename");
                    total += self.file_frames(&name);
                }
            }
            return total;
        }
        for key in ["TilesetFilenamesPattern", "FilenamePattern"] {
            if let Some(p) = node_d(&self.data, &self.defaults, key) {
                let pat = if key == "TilesetFilenamesPattern" {
                    node(&p.value, &self.tileset).map(|n| &n.value)
                } else {
                    Some(&p.value)
                };
                if let Some(pat) = pat {
                    return node(pat, "Count").map_or(1, |n| get_value::<i32>("Count", n.value.value.as_deref()).expect("Count"));
                }
            }
        }
        if let Some(f) = node_d(&self.data, &self.defaults, "Frames") {
            return get_value::<Vec<i32>>("Frames", f.value.value.as_deref()).expect("Frames").len() as i32;
        }
        let name = self
            .filename_for_tileset(&self.data)
            .or_else(|| node_d(&self.data, &self.defaults, "Filename").map(|n| n.value.value_str().to_string()))
            .unwrap_or_else(|| panic!("sequence {}.{} has no filename", self.image, self.name));
        self.file_frames(&name)
    }

    /// `ISpriteSequence.Length`, after the doubling of reversing sequences.
    pub fn length(&self) -> i32 {
        *self.resolved_length.get_or_init(|| {
            let l = self.declared_length.unwrap_or_else(|| self.all_sprites_count() - self.start);
            if self.reverses { 2 * l - 2 } else { l }
        })
    }
}

/// `SequenceSet`.
pub struct SequenceSet {
    images: HashMap<String, indexmap::IndexMap<String, Rc<Sequence>>>,
}

impl SequenceSet {
    pub fn new(nodes: &[MiniYamlNode], tileset: &str, read_file: ReadFile) -> SequenceSet {
        let empty = MiniYaml::new(None, Vec::new());
        let mut images = HashMap::new();
        for img in nodes {
            if img.key.starts_with('^') {
                continue;
            }
            let defaults = node(&img.value, "Defaults").map_or(&empty, |n| &n.value);
            let mut seqs = indexmap::IndexMap::new();
            for s in &img.value.nodes {
                if s.key == "Defaults" {
                    continue;
                }
                seqs.insert(
                    s.key.clone(),
                    Rc::new(Sequence::new(&img.key, &s.key, &s.value, defaults, tileset, read_file.clone())),
                );
            }
            images.insert(img.key.clone(), seqs);
        }
        SequenceSet { images }
    }

    pub fn get_sequence(&self, image: &str, sequence: &str) -> Rc<Sequence> {
        let seqs = self
            .images
            .get(image)
            .unwrap_or_else(|| panic!("Image `{image}` does not have any sequences defined."));
        seqs.get(sequence)
            .cloned()
            .unwrap_or_else(|| panic!("Image `{image}` does not have a sequence named `{sequence}`."))
    }

    pub fn has_sequence(&self, image: &str, sequence: &str) -> bool {
        self.images
            .get(image)
            .unwrap_or_else(|| panic!("Image `{image}` does not have any sequences defined."))
            .contains_key(sequence)
    }

    /// `SequenceSet.Sequences`: the sequence names of an image.
    pub fn sequence_names(&self, image: &str) -> Vec<String> {
        self.images.get(image).map_or_else(Vec::new, |s| s.keys().cloned().collect())
    }

    pub fn has_image(&self, image: &str) -> bool {
        self.images.contains_key(image)
    }
}

/// The function that runs after an animation, either a closure, which save games cannot keep, or a
/// [`crate::persist::SavedFn`].
#[derive(Clone)]
pub enum AfterAnim {
    Closure(Rc<dyn Fn()>),
    Saved(Rc<dyn crate::persist::SavedFn>),
}

/// A closure cannot be saved: it is dropped with a warning and loads as a function that does nothing.
impl crate::persist::Persist for AfterAnim {
    fn save(&self, w: &mut crate::persist::SaveWriter) {
        match self {
            AfterAnim::Saved(f) => {
                w.u8(1);
                f.save(w);
            }
            AfterAnim::Closure(_) => {
                log::warn!("save game: a function after an animation is a closure and is dropped");
                w.u8(0);
            }
        }
    }
    fn load(r: &mut crate::persist::SaveReader) -> Self {
        match r.u8() {
            1 => AfterAnim::Saved(crate::persist::Persist::load(r)),
            0 => AfterAnim::Closure(Rc::new(|| {})),
            t => r.fail(format!("invalid animation function tag {t}")),
        }
    }
}

impl AfterAnim {
    /// A function that save games keep.
    pub fn saved(f: impl crate::persist::SavedFn + 'static) -> AfterAnim {
        AfterAnim::Saved(Rc::new(f))
    }

    pub fn call(&self) {
        match self {
            AfterAnim::Closure(f) => f(),
            AfterAnim::Saved(f) => f.call(),
        }
    }
}

#[derive(Clone)]
enum TickFunc {
    None,
    Repeating,
    Then(Option<AfterAnim>),
    FetchIndex(Rc<dyn Fn() -> i32>),
    /// `PlayFetchIndex` with a function that save games keep.
    FetchSaved(Rc<dyn crate::persist::SavedIndex>),
    FetchDirection(Rc<dyn Fn() -> i32>),
}

/// `Graphics/Animation.cs`.
pub struct Animation {
    pub name: RefCell<String>,
    pub is_decoration: Cell<bool>,
    sequences: Rc<SequenceSet>,
    pub facing_func: Rc<dyn Fn() -> WAngle>,
    paused: Option<Rc<dyn Fn() -> bool>>,
    current: RefCell<Option<Rc<Sequence>>>,
    frame: Cell<i32>,
    backwards: Cell<bool>,
    tick_always: Cell<bool>,
    time_until_next_frame: Cell<i32>,
    tick_func: RefCell<TickFunc>,
}

impl Animation {
    pub fn new(
        sequences: Rc<SequenceSet>,
        name: &str,
        facing_func: Option<Rc<dyn Fn() -> WAngle>>,
        paused: Option<Rc<dyn Fn() -> bool>>,
    ) -> Rc<Animation> {
        Rc::new(Animation {
            name: RefCell::new(name.to_lowercase()),
            is_decoration: Cell::new(false),
            sequences,
            facing_func: facing_func.unwrap_or_else(|| Rc::new(|| WAngle::ZERO)),
            paused,
            current: RefCell::new(None),
            frame: Cell::new(0),
            backwards: Cell::new(false),
            tick_always: Cell::new(false),
            time_until_next_frame: Cell::new(0),
            tick_func: RefCell::new(TickFunc::None),
        })
    }

    pub fn current_sequence(&self) -> Option<Rc<Sequence>> {
        self.current.borrow().clone()
    }

    fn current(&self) -> Rc<Sequence> {
        self.current.borrow().clone().expect("animation has a sequence")
    }

    pub fn current_frame(&self) -> i32 {
        if self.backwards.get() {
            self.current().length() - self.frame.get() - 1
        } else {
            self.frame.get()
        }
    }

    fn tick_or_default(&self) -> i32 {
        self.current.borrow().as_ref().map_or(40, |s| s.tick)
    }

    fn play_sequence(&self, name: &str) {
        *self.current.borrow_mut() = Some(self.get_sequence(name));
        self.time_until_next_frame.set(self.tick_or_default());
    }

    pub fn play(&self, name: &str) {
        self.play_then(name, None);
    }

    pub fn play_repeating(&self, name: &str) {
        self.backwards.set(false);
        self.tick_always.set(false);
        self.play_sequence(name);
        self.frame.set(0);
        *self.tick_func.borrow_mut() = TickFunc::Repeating;
    }

    pub fn replace_anim(&self, name: &str) -> bool {
        if !self.has_sequence(name) {
            return false;
        }
        *self.current.borrow_mut() = Some(self.get_sequence(name));
        self.time_until_next_frame
            .set(self.tick_or_default().min(self.time_until_next_frame.get()));
        self.frame.set(self.frame.get() % self.current().length());
        true
    }

    pub fn play_then(&self, name: &str, after: Option<Rc<dyn Fn()>>) {
        self.play_then_after(name, after.map(AfterAnim::Closure));
    }

    /// `PlayThen` with a function that save games keep.
    pub fn play_then_saved(&self, name: &str, after: Rc<dyn crate::persist::SavedFn>) {
        self.play_then_after(name, Some(AfterAnim::Saved(after)));
    }

    pub fn play_then_after(&self, name: &str, after: Option<AfterAnim>) {
        self.backwards.set(false);
        self.tick_always.set(false);
        self.play_sequence(name);
        self.frame.set(0);
        *self.tick_func.borrow_mut() = TickFunc::Then(after);
    }

    pub fn play_backwards_then(&self, name: &str, after: Option<Rc<dyn Fn()>>) {
        self.play_backwards_then_after(name, after.map(AfterAnim::Closure));
    }

    pub fn play_backwards_then_after(&self, name: &str, after: Option<AfterAnim>) {
        self.play_then_after(name, after);
        self.backwards.set(true);
    }

    pub fn play_fetch_index(&self, name: &str, func: Rc<dyn Fn() -> i32>) {
        self.backwards.set(false);
        self.tick_always.set(true);
        self.play_sequence(name);
        self.frame.set(func());
        *self.tick_func.borrow_mut() = TickFunc::FetchIndex(func);
    }

    /// `PlayFetchIndex` with a function that save games keep.
    pub fn play_fetch_index_saved(&self, name: &str, func: Rc<dyn crate::persist::SavedIndex>) {
        self.backwards.set(false);
        self.tick_always.set(true);
        self.play_sequence(name);
        self.frame.set(func.index());
        *self.tick_func.borrow_mut() = TickFunc::FetchSaved(func);
    }

    pub fn play_fetch_direction(&self, name: &str, direction: Rc<dyn Fn() -> i32>) {
        self.tick_always.set(false);
        self.play_sequence(name);
        self.frame.set(0);
        *self.tick_func.borrow_mut() = TickFunc::FetchDirection(direction);
    }

    fn invoke_tick_func(&self) {
        let f = self.tick_func.borrow().clone();
        match f {
            TickFunc::None => {}
            TickFunc::Repeating => {
                self.frame.set(self.frame.get() + 1);
                if self.frame.get() >= self.current().length() {
                    self.frame.set(0);
                }
            }
            TickFunc::Then(after) => {
                self.frame.set(self.frame.get() + 1);
                let len = self.current().length();
                if self.frame.get() >= len {
                    self.frame.set(len - 1);
                    *self.tick_func.borrow_mut() = TickFunc::None;
                    if let Some(a) = after {
                        a.call();
                    }
                }
            }
            TickFunc::FetchIndex(func) => self.frame.set(func()),
            TickFunc::FetchSaved(func) => self.frame.set(func.index()),
            TickFunc::FetchDirection(direction) => {
                let d = direction();
                if d > 0 {
                    self.frame.set(self.frame.get() + 1);
                    if self.frame.get() >= self.current().length() {
                        self.frame.set(0);
                    }
                }
                if d < 0 {
                    self.frame.set(self.frame.get() - 1);
                    if self.frame.get() < 0 {
                        self.frame.set(self.current().length() - 1);
                    }
                }
            }
        }
    }

    pub fn tick(&self) {
        if self.paused.as_ref().is_none_or(|p| !p()) {
            self.tick_by(40);
        }
    }

    pub fn tick_by(&self, t: i32) {
        if self.tick_always.get() {
            self.invoke_tick_func();
        } else {
            self.time_until_next_frame.set(self.time_until_next_frame.get() - t);
            while self.time_until_next_frame.get() <= 0 {
                self.invoke_tick_func();
                self.time_until_next_frame
                    .set(self.time_until_next_frame.get() + self.tick_or_default());
            }
        }
    }

    pub fn change_image(&self, new_image: &str, new_anim_if_missing: &str) {
        let new_image = new_image.to_lowercase();
        if *self.name.borrow() != new_image {
            *self.name.borrow_mut() = new_image;
            let cur = self.current().name.clone();
            if !self.replace_anim(&cur) {
                self.replace_anim(new_anim_if_missing);
            }
        }
    }

    pub fn has_sequence(&self, seq: &str) -> bool {
        self.sequences.has_sequence(&self.name.borrow(), seq)
    }

    pub fn get_sequence(&self, name: &str) -> Rc<Sequence> {
        self.sequences.get_sequence(&self.name.borrow(), name)
    }

    /// `GetRandomExistingSequence`.
    pub fn get_random_existing_sequence(
        &self,
        sequences: &[String],
        random: &mut rustora_core::world_types::MersenneTwister,
    ) -> Option<String> {
        let v: Vec<&String> = sequences.iter().filter(|s| self.has_sequence(s)).collect();
        if v.is_empty() {
            return None;
        }
        Some(v[random.next_max(v.len() as i32) as usize].clone())
    }

    pub fn sequences(&self) -> &Rc<SequenceSet> {
        &self.sequences
    }
}

/// The animation state is written in place. The facing and pause functions and the fetch functions of
/// `PlayFetchIndex` and `PlayFetchDirection` are set up by the owner when it is created, so the loaded animation
/// keeps its own. A closure passed to `PlayThen` cannot be saved and is dropped with a warning.
impl crate::persist::PersistState for Animation {
    fn save_state(&self, w: &mut crate::persist::SaveWriter) {
        use crate::persist::Persist;
        self.name.save(w);
        self.is_decoration.save(w);
        self.current.borrow().as_ref().map(|s| s.name.clone()).save(w);
        self.frame.save(w);
        self.backwards.save(w);
        self.tick_always.save(w);
        self.time_until_next_frame.save(w);
        match &*self.tick_func.borrow() {
            TickFunc::None => w.u8(0),
            TickFunc::Repeating => w.u8(1),
            TickFunc::Then(None) => w.u8(2),
            TickFunc::Then(Some(f)) => {
                w.u8(3);
                f.save(w);
            }
            TickFunc::FetchIndex(_) => w.u8(4),
            TickFunc::FetchSaved(f) => {
                w.u8(6);
                f.save(w);
            }
            TickFunc::FetchDirection(_) => w.u8(5),
        }
    }

    fn load_state(&self, r: &mut crate::persist::SaveReader) {
        use crate::persist::Persist;
        self.name.load_state(r);
        self.is_decoration.load_state(r);
        let current: Option<String> = Persist::load(r);
        *self.current.borrow_mut() = current.map(|s| self.get_sequence(&s));
        self.frame.load_state(r);
        self.backwards.load_state(r);
        self.tick_always.load_state(r);
        self.time_until_next_frame.load_state(r);
        let kind = r.u8();
        let keep = matches!(
            (kind, &*self.tick_func.borrow()),
            (4, TickFunc::FetchIndex(_)) | (5, TickFunc::FetchDirection(_))
        );
        let f = match kind {
            0 => TickFunc::None,
            1 => TickFunc::Repeating,
            2 => TickFunc::Then(None),
            3 => TickFunc::Then(Some(Persist::load(r))),
            6 => TickFunc::FetchSaved(Persist::load(r)),
            4 | 5 if keep => return,
            4 | 5 => {
                log::warn!("save game: the animation {} has no fetch function to restore", self.name.borrow());
                TickFunc::None
            }
            k => r.fail(format!("invalid animation tick function {k}")),
        };
        *self.tick_func.borrow_mut() = f;
    }
}

impl crate::persist::OwnedState for Animation {}

/// `AnimationWithOffset`.
#[derive(Clone, crate::persist::Persist)]
pub struct AnimationWithOffset {
    pub animation: Rc<Animation>,
    pub offset_func: Option<Rc<dyn Fn() -> rustora_core::world_types::WVec>>,
    pub disable_func: Option<Rc<dyn Fn() -> bool>>,
    pub zoffset_func: Option<Rc<dyn Fn(rustora_core::world_types::WPos) -> i32>>,
}
