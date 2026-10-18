//! Background rendering of batch labels (port of `web/src/ui/batch/batch-measure.svelte.ts`).
//!
//! Renders batch labels off the critical path, one at a time: first the thumbnails the preview
//! grid shows (the current page), then every other label to MEASURE its length, so the tape
//! estimate becomes exact. Results are keyed by (doc, model, tape): any edit cancels the run,
//! clears the results and starts again after a short pause. Only the thumbnails of the page on
//! screen are kept.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use baradar_core::doc::LabelDoc;
use baradar_core::render::renderer::{RenderOptions, RenderResult, RenderTarget};
use baradar_core::render::units::css_to_rgb;
use baradar_core::vars::variables::{VariableContext, batch_size, resolve_doc};
use gpui_kit::*;

use super::BatchPanel;
use crate::studio::local_today;
use crate::view_model::DEFAULT_COLORS;

/// Wait after an edit before rendering again (typing in a cell re-keys every keystroke).
const SETTLE: Duration = Duration::from_millis(350);

/// Non-blocking warnings worth flagging per batch row, most important first: a long value cut
/// off, or shrunk too far to read (`ROW_NOTICES`).
const ROW_NOTICES: [&str; 2] = ["content-overflow", "small-text"];

/// What one rendered batch label measured (`RowInfo`).
#[derive(Debug, Clone)]
pub struct RowInfo {
    pub length_mm: f64,
    /// The label cannot be printed (first blocking warning).
    pub problem: Option<String>,
    /// It prints, but not as designed: text cut off or too small to read (first such warning).
    pub notice: Option<String>,
}

/// One label to render.
struct Job {
    row: usize,
    thumb: bool,
    doc: Arc<LabelDoc>,
    target: RenderTarget,
    colors: (u32, u32),
}

/// What a finished job produced.
type JobOutput = Option<(RowInfo, f64, Option<Arc<RenderImage>>)>;

/// Measuring state of the batch panel (`BatchMeasure`).
#[derive(Default)]
pub struct BatchMeasure {
    key: Option<(LabelDoc, &'static str, &'static str)>,
    doc: Option<Arc<LabelDoc>>,
    target: Option<RenderTarget>,
    colors: (u32, u32),
    generation: u64,
    /// Measured labels by row.
    pub rows: HashMap<usize, RowInfo>,
    /// Thumbnails of the rows on the current preview page (`None`: the label has no dots).
    pub thumbs: HashMap<usize, Option<Arc<RenderImage>>>,
    /// Labels of the current batch.
    pub total: usize,
    pub feed_margin_mm: f64,
    /// Rows whose thumbnails are wanted (the preview page).
    want: Range<usize>,
    running: bool,
    cancel: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
}

impl BatchMeasure {
    /// Measured length per label (`None` = not yet), for `batch_tape`.
    pub fn lengths(&self) -> Vec<Option<f64>> {
        (0..self.total).map(|i| self.rows.get(&i).map(|r| r.length_mm)).collect()
    }

    /// Labels that cannot be printed, by row (row, message), in row order.
    pub fn problems(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .rows
            .iter()
            .filter_map(|(i, r)| r.problem.clone().map(|p| (*i, p)))
            .collect();
        out.sort_by_key(|(i, _)| *i);
        out
    }

    /// Labels that print, but not as designed (row, message), in row order.
    pub fn notices(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .rows
            .iter()
            .filter(|(_, r)| r.problem.is_none())
            .filter_map(|(i, r)| r.notice.clone().map(|p| (*i, p)))
            .collect();
        out.sort_by_key(|(i, _)| *i);
        out
    }

    fn stop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.task = None;
        self.running = false;
    }
}

/// Tape and ink colours of the thumbnails: the doc's preview colours, else black on white.
fn thumb_colors(doc: &LabelDoc) -> (u32, u32) {
    match &doc.tape.colors {
        Some(c) => (css_to_rgb(&c.tape), css_to_rgb(&c.ink)),
        None => (css_to_rgb(DEFAULT_COLORS.0), css_to_rgb(DEFAULT_COLORS.1)),
    }
}

/// The exact dots of a render as a GPUI image (BGRA), or `None` for an empty label.
pub fn bitmap_image(bitmap: &ptouch::Bitmap, (tape, ink): (u32, u32)) -> Option<Arc<RenderImage>> {
    if bitmap.length() == 0 || bitmap.height() == 0 {
        return None;
    }
    let mut rgba = bitmap.to_rgba(tape, ink);
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(bitmap.length(), u32::from(bitmap.height()), rgba)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

fn row_info(r: &RenderResult) -> RowInfo {
    let problem = r.warnings.iter().find(|w| w.blocking).map(|w| w.message.clone());
    let notice = ROW_NOTICES
        .iter()
        .find_map(|code| r.warnings.iter().find(|w| w.code == *code))
        .map(|w| w.message.clone());
    RowInfo {
        length_mm: r.length_mm,
        problem,
        notice,
    }
}

impl BatchPanel {
    /// Follows the doc and target (`measure.sync`): a changed key restarts measuring.
    pub(super) fn sync_measure(&mut self, cx: &mut Context<Self>) {
        let s = self.studio.read(cx);
        let total = batch_size(&s.doc);
        let target = s.target().ok();
        let Some(target) = target.filter(|_| total > 0) else {
            if self.measure.key.is_some() {
                self.measure.stop();
                self.measure = BatchMeasure::default();
            }
            return;
        };
        let key = (s.doc.clone(), target.profile.name, target.tape.id);
        if self.measure.key.as_ref() == Some(&key) {
            return;
        }
        let doc = Arc::new(s.doc.clone());
        let m = &mut self.measure;
        m.stop();
        m.generation += 1;
        m.rows.clear();
        m.thumbs.clear();
        m.total = total;
        m.colors = thumb_colors(&doc);
        m.doc = Some(doc);
        m.target = Some(target);
        m.key = Some(key);
        self.start_measure(true, cx);
    }

    /// The rows the preview grid shows now: their thumbnails are rendered first.
    pub(super) fn want_thumbs(&mut self, rows: Range<usize>, cx: &mut Context<Self>) {
        if self.measure.want == rows {
            return;
        }
        self.measure.thumbs.retain(|r, _| rows.contains(r));
        self.measure.want = rows;
        if !self.measure.running && self.measure.doc.is_some() {
            self.start_measure(false, cx);
        }
    }

    /// The next label to render: missing thumbnails first, then unmeasured labels.
    fn next_measure_job(&self, generation: u64) -> Option<Job> {
        let m = &self.measure;
        if m.generation != generation {
            return None;
        }
        let (doc, target) = (m.doc.clone()?, m.target?);
        let thumb_row = m.want.clone().filter(|r| *r < m.total).find(|r| !m.thumbs.contains_key(r));
        let (row, thumb) = match thumb_row {
            Some(r) => (r, true),
            None => ((0..m.total).find(|r| !m.rows.contains_key(r))?, false),
        };
        Some(Job {
            row,
            thumb,
            doc,
            target,
            colors: m.colors,
        })
    }

    fn store_measure(&mut self, generation: u64, row: usize, thumb: bool, out: JobOutput, cx: &mut Context<Self>) {
        let m = &mut self.measure;
        if m.generation != generation {
            return;
        }
        match out {
            Some((info, feed, image)) => {
                m.feed_margin_mm = feed;
                m.rows.insert(row, info);
                if thumb && m.want.contains(&row) {
                    m.thumbs.insert(row, image);
                }
            }
            None => {
                m.rows.insert(
                    row,
                    RowInfo {
                        length_mm: 0.0,
                        problem: Some("The label could not be rendered.".into()),
                        notice: None,
                    },
                );
            }
        }
        if thumb && m.want.contains(&row) {
            m.thumbs.entry(row).or_insert(None);
        }
        cx.notify();
    }

    fn start_measure(&mut self, settle: bool, cx: &mut Context<Self>) {
        let generation = self.measure.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(c) = self.measure.cancel.replace(cancel.clone()) {
            c.store(true, Ordering::Relaxed);
        }
        self.measure.running = true;
        let renderer = self.studio.read(cx).renderer.clone();
        self.measure.task = Some(cx.spawn(async move |this, cx| {
            if settle {
                cx.background_executor().timer(SETTLE).await;
            }
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let job = this.update(cx, |p, _| p.next_measure_job(generation)).ok().flatten();
                let Some(job) = job else { break };
                let renderer = renderer.clone();
                let flag = cancel.clone();
                let (row, thumb) = (job.row, job.thumb);
                let out: JobOutput = cx
                    .background_executor()
                    .spawn(async move {
                        let ctx = VariableContext::new(local_today());
                        let resolved = resolve_doc(&job.doc, job.row, &ctx).doc.into_owned();
                        let opts = RenderOptions {
                            factor: 0,
                            cancel: Some(&flag),
                        };
                        let r = renderer.render(&resolved, &job.target, &opts).ok()?;
                        let image = if job.thumb { bitmap_image(&r.bitmap, job.colors) } else { None };
                        Some((row_info(&r), r.feed_margin_mm, image))
                    })
                    .await;
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if this
                    .update(cx, |p, cx| p.store_measure(generation, row, thumb, out, cx))
                    .is_err()
                {
                    return;
                }
            }
            this.update(cx, |p, _| {
                if p.measure.generation == generation {
                    p.measure.running = false;
                }
            })
            .ok();
        }));
    }
}
