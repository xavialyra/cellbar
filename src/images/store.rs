use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender as TaskSender, TrySendError},
    },
    thread,
    time::Duration,
};

use image::{ImageFormat, ImageReader, Limits, RgbaImage, imageops::FilterType};
use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 2048;
const MAX_PIXELS: u64 = 2 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 512 * 1024;
const MAX_PREVIEW_DIMENSION: u32 = 256;
const MAX_ENTRIES: usize = 64;
const IDLE_WORKER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key(PathBuf, u64, u32, u32);

enum Status {
    Loading(u64),
    Ready(u64, u32, u32),
    Failed,
}

enum Task {
    Load(PathBuf, u64),
    Resize(Key),
}

pub enum ResultMessage {
    Loaded(PathBuf, u64, Result<(u32, u32), String>),
    Resized(PathBuf, u64, u32, u32, Result<RgbaImage, String>),
}

pub struct ImageStore {
    sender: SyncSender<ResultMessage>,
    worker: Option<TaskSender<Task>>,
    worker_thread: Option<thread::JoinHandle<()>>,
    worker_stop: Option<Arc<AtomicBool>>,
    entries: HashMap<PathBuf, Status>,
    scaled: HashMap<Key, Arc<RgbaImage>>,
    pending: std::collections::HashSet<Key>,
    generation: u64,
    revision: u64,
}

impl Drop for ImageStore {
    fn drop(&mut self) {
        if let Some(stop) = self.worker_stop.take() {
            stop.store(true, Ordering::Release);
        }
        self.worker.take();
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
    }
}

impl ImageStore {
    pub fn new(sender: SyncSender<ResultMessage>) -> Self {
        Self {
            sender,
            worker: None,
            worker_thread: None,
            worker_stop: None,
            entries: HashMap::new(),
            scaled: HashMap::new(),
            pending: Default::default(),
            generation: 0,
            revision: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn submit(&mut self, mut task: Task) -> bool {
        if let Some(worker) = &self.worker {
            match worker.try_send(task) {
                Ok(()) => return true,
                Err(TrySendError::Full(_)) => return false,
                Err(TrySendError::Disconnected(recovered_task)) => {
                    self.worker = None;
                    if let Some(handle) = self.worker_thread.take() {
                        let _ = handle.join();
                    }
                    self.worker_stop = None;
                    task = recovered_task;
                }
            }
        }

        let (sender, receiver) = mpsc::sync_channel(32);
        let notify = self.sender.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let Ok(handle) = thread::Builder::new()
            .name("cellbar-images".into())
            .stack_size(256 * 1024)
            .spawn(move || worker(receiver, notify, worker_stop))
        else {
            eprintln!("cellbar: cannot start image decoder");
            return false;
        };
        let ok = sender.try_send(task).is_ok();
        self.worker = Some(sender);
        self.worker_thread = Some(handle);
        self.worker_stop = Some(stop);
        ok
    }

    pub fn dimensions(&mut self, src: &PathBuf) -> Option<(u32, u32)> {
        match self.entries.get(src) {
            Some(Status::Ready(_, w, h)) => return Some((*w, *h)),
            Some(Status::Loading(_) | Status::Failed) => return None,
            None => {}
        }
        self.revision = self.revision.wrapping_add(1);
        let revision = self.revision;
        self.ensure_entry_capacity();
        if self.submit(Task::Load(src.clone(), revision)) {
            self.entries.insert(src.clone(), Status::Loading(revision));
        }
        None
    }

    pub fn scaled(&mut self, src: &PathBuf, width: u32, height: u32) -> Option<Arc<RgbaImage>> {
        let Status::Ready(rev, _, _) = self.entries.get(src)? else {
            return None;
        };
        let key = Key(src.clone(), *rev, width, height);
        if let Some(image) = self.scaled.get(&key) {
            return Some(image.clone());
        }
        if self.pending.contains(&key) {
            return None;
        }
        if self.submit(Task::Resize(key.clone())) {
            self.pending.insert(key);
        }
        None
    }

    fn ensure_entry_capacity(&mut self) {
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.clear();
            self.scaled.clear();
            self.pending.clear();
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.scaled.clear();
        self.pending.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn refresh(&mut self, src: &PathBuf) {
        self.entries.remove(src);
        self.scaled.retain(|key, _| &key.0 != src);
        self.pending.retain(|key| &key.0 != src);
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn complete(&mut self, result: ResultMessage) {
        match result {
            ResultMessage::Loaded(src, rev, result) => {
                if !matches!(self.entries.get(&src), Some(Status::Loading(current)) if *current == rev)
                {
                    return;
                }
                match result {
                    Ok((w, h)) => {
                        self.entries.insert(src, Status::Ready(rev, w, h));
                    }
                    Err(error) => {
                        eprintln!("cellbar: cannot load image {}: {error}", src.display());
                        self.entries.insert(src, Status::Failed);
                    }
                }
            }
            ResultMessage::Resized(src, rev, width, height, result) => {
                let key = Key(src.clone(), rev, width, height);
                if !self.pending.remove(&key)
                    || !matches!(self.entries.get(&src), Some(Status::Ready(current, _, _)) if *current == rev)
                {
                    return;
                }
                match result {
                    Ok(image) => {
                        let size = image.as_raw().len();
                        if size <= MAX_CACHE_BYTES {
                            let used: usize =
                                self.scaled.values().map(|image| image.as_raw().len()).sum();
                            if used.saturating_add(size) > MAX_CACHE_BYTES {
                                self.scaled.clear();
                            }
                            self.scaled.insert(key, Arc::new(image));
                        } else {
                            self.entries.insert(src.clone(), Status::Failed);
                            eprintln!(
                                "cellbar: scaled image {} exceeds cache limit",
                                src.display()
                            );
                        }
                    }
                    Err(error) => {
                        self.entries.insert(src.clone(), Status::Failed);
                        eprintln!("cellbar: cannot resize image {}: {error}", src.display());
                    }
                }
            }
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

fn worker(
    receiver: mpsc::Receiver<Task>,
    notify: SyncSender<ResultMessage>,
    stop: Arc<AtomicBool>,
) {
    let mut originals: HashMap<PathBuf, (u64, RgbaImage)> = HashMap::new();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let task = match receiver.recv_timeout(IDLE_WORKER_TIMEOUT) {
            Ok(task) => task,
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match task {
            Task::Load(src, rev) => {
                let result = decode(&src).map(|(image, dimensions)| {
                    let used: usize = originals
                        .values()
                        .map(|(_, image)| image.as_raw().len())
                        .sum();
                    if used.saturating_add(image.as_raw().len()) > MAX_CACHE_BYTES {
                        originals.clear();
                    }
                    originals.insert(src.clone(), (rev, image));
                    dimensions
                });
                if !notify_result(&notify, &stop, ResultMessage::Loaded(src, rev, result)) {
                    break;
                }
            }
            Task::Resize(Key(src, rev, width, height)) => {
                let result = match originals.get(&src) {
                    Some((current, image)) if *current == rev => Ok(image::imageops::resize(
                        image,
                        width,
                        height,
                        FilterType::Triangle,
                    )),
                    _ => decode(&src).map(|(image, _)| {
                        image::imageops::resize(&image, width, height, FilterType::Triangle)
                    }),
                };
                if !notify_result(
                    &notify,
                    &stop,
                    ResultMessage::Resized(src, rev, width, height, result),
                ) {
                    break;
                }
            }
        }
    }
}

fn notify_result(
    notify: &SyncSender<ResultMessage>,
    stop: &AtomicBool,
    mut result: ResultMessage,
) -> bool {
    loop {
        match notify.try_send(result) {
            Ok(()) => return true,
            Err(TrySendError::Full(next)) => {
                result = next;
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn decode(src: &PathBuf) -> Result<(RgbaImage, (u32, u32)), String> {
    let file = File::open(src).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err("file exceeds 8 MiB".into());
    }
    let mut signature = [0; 32];
    let mut probe = BufReader::new(File::open(src).map_err(|e| e.to_string())?);
    let count = probe
        .read(&mut signature)
        .map_err(|error| error.to_string())?;
    let format = image::guess_format(&signature[..count]).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err("unsupported image format".into());
    }
    if format == ImageFormat::WebP {
        let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(
            File::open(src).map_err(|e| e.to_string())?,
        ))
        .map_err(|error| error.to_string())?;
        if decoder.has_animation() {
            return Err("animated WebP is not supported".into());
        }
    }
    if format == ImageFormat::Png {
        let decoder = image::codecs::png::PngDecoder::new(BufReader::new(
            File::open(src).map_err(|e| e.to_string())?,
        ))
        .map_err(|error| error.to_string())?;
        if decoder.is_apng().map_err(|error| error.to_string())? {
            return Err("animated PNG is not supported".into());
        }
    }
    let dimensions = ImageReader::with_format(
        BufReader::new(File::open(src).map_err(|e| e.to_string())?),
        format,
    )
    .into_dimensions()
    .map_err(|error| error.to_string())?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_DIMENSION
        || dimensions.1 > MAX_DIMENSION
        || u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_PIXELS
    {
        return Err("image dimensions exceed limit".into());
    }
    let mut reader = ImageReader::with_format(BufReader::new(file), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader
        .decode()
        .map_err(|error| error.to_string())?
        .to_rgba8();

    let (w, h) = image.dimensions();
    let max_side = w.max(h);
    if max_side > MAX_PREVIEW_DIMENSION {
        let (new_w, new_h) = if w >= h {
            (
                MAX_PREVIEW_DIMENSION,
                ((h as u64 * MAX_PREVIEW_DIMENSION as u64) / w as u64).max(1) as u32,
            )
        } else {
            (
                ((w as u64 * MAX_PREVIEW_DIMENSION as u64) / h as u64).max(1) as u32,
                MAX_PREVIEW_DIMENSION,
            )
        };
        image = image::imageops::resize(&image, new_w, new_h, FilterType::Triangle);
    }

    Ok((image, dimensions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_store_loads_and_resizes_off_thread() {
        use smithay_client_toolkit::reexports::calloop::{
            EventLoop,
            channel::{self, Event},
        };
        use std::time::Duration;
        let path = std::env::temp_dir().join(format!("cellbar-store-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let (sender, receiver) = channel::sync_channel(8);
        let mut loop_: EventLoop<Vec<ResultMessage>> = EventLoop::try_new().unwrap();
        loop_
            .handle()
            .insert_source(receiver, |event, _, results| {
                if let Event::Msg(result) = event {
                    results.push(result);
                }
            })
            .unwrap();
        let mut store = ImageStore::new(sender);
        let mut results = Vec::new();
        assert_eq!(store.dimensions(&path), None);
        loop_
            .dispatch(Duration::from_secs(2), &mut results)
            .unwrap();
        for result in results.drain(..) {
            store.complete(result);
        }
        assert_eq!(store.dimensions(&path), Some((2, 2)));
        assert!(store.scaled(&path, 4, 4).is_none());
        loop_
            .dispatch(Duration::from_secs(2), &mut results)
            .unwrap();
        for result in results.drain(..) {
            store.complete(result);
        }
        let resized = store.scaled(&path, 4, 4).unwrap();
        assert_eq!(resized.dimensions(), (4, 4));
        store.refresh(&path);
        assert!(store.dimensions(&path).is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn entry_limit_and_blocked_result_queue_remain_bounded() {
        use smithay_client_toolkit::reexports::calloop::channel;

        let (sender, _receiver) = channel::sync_channel(1);
        let mut store = ImageStore::new(sender);
        for index in 0..MAX_ENTRIES {
            let path = PathBuf::from(format!("/nonexistent/cellbar-image-{index}"));
            store.entries.insert(path, Status::Failed);
        }
        store.dimensions(&PathBuf::from("/nonexistent/new-image"));
        assert_eq!(store.entries.len(), 1);
        for index in 0..32 {
            store.dimensions(&PathBuf::from(format!("/nonexistent/pending-{index}")));
        }
        drop(store);
    }

    #[test]
    fn decodes_static_png_and_rejects_large_source() {
        let path = std::env::temp_dir().join(format!("cellbar-image-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 128]))
            .save(&path)
            .unwrap();
        let (image, dims) = decode(&path).unwrap();
        assert_eq!(dims, (2, 2));
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 128]);
        std::fs::remove_file(&path).unwrap();
        assert!(decode(&path).is_err());
    }
}
