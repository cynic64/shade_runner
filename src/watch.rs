use crate::error::Error;
use crate::layouts::Entry;
use crate::CompiledShaders;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

pub struct Watch {
    _handler: Handler,
    pub rx: Receiver<Result<Message, Error>>,
}

enum Loader {
    Graphics(GraphicsLoader),
    Compute(ComputeLoader),
}

enum SrcPath {
    Graphics(PathBuf, PathBuf),
    Compute(PathBuf),
}

struct GraphicsLoader {
    vertex: PathBuf,
    fragment: PathBuf,
    tx: Sender<Result<Message, Error>>,
}

struct ComputeLoader {
    compute: PathBuf,
    tx: Sender<Result<Message, Error>>,
}

pub struct Message {
    pub shaders: CompiledShaders,
    pub entry: Entry,
}

impl Watch {
    /// Paths to the vertex and fragment shaders.
    /// Frequency is how often the watcher will check the directory.
    pub fn create<T>(vertex: T, fragment: T, frequency: Duration) -> Result<Self, Error>
    where
        T: AsRef<Path>,
    {
        let src_path = SrcPath::Graphics(
            vertex.as_ref().to_path_buf(),
            fragment.as_ref().to_path_buf()
            );
        let (handler, rx) = create_watch(
            src_path,
            frequency,
        )?;
        Ok(Watch {
            _handler: handler,
            rx,
        })
    }

    pub fn create_compute<T>(compute: T, frequency: Duration) -> Result<Self, Error>
    where
        T: AsRef<Path>,
    {
        let src_path = SrcPath::Compute(
            compute.as_ref(). to_path_buf());
        let (handler, rx) = create_watch(
            src_path,
            frequency,
        )?;
        Ok(Watch {
            _handler: handler,
            rx,
        })
    }
}

impl GraphicsLoader {
    fn create(vertex: PathBuf, fragment: PathBuf) -> (Self, Receiver<Result<Message, Error>>) {
        let (tx, rx) = mpsc::channel();
        let loader = GraphicsLoader {
            vertex,
            fragment,
            tx,
        };
        loader.reload();
        (loader, rx)
    }

    fn reload(&self) {
        match crate::load(&self.vertex, &self.fragment) {
            Ok(shaders) => {
                let entry = crate::parse(&shaders);
                let msg = entry.map(|entry| Message { shaders, entry });
                self.tx.send(msg).ok()
            }
            Err(e) => self.tx.send(Err(e)).ok(),
        };
    }
}

impl ComputeLoader {
    fn create(compute: PathBuf) -> (Self, Receiver<Result<Message, Error>>) {
        let (tx, rx) = mpsc::channel();
        let loader = ComputeLoader {
            compute,
            tx,
        };
        loader.reload();
        (loader, rx)
    }

    fn reload(&self) {
        match crate::load_compute(&self.compute) {
            Ok(shaders) => {
                let entry = crate::parse_compute(&shaders);
                let msg = entry.map(|entry| Message { shaders, entry });
                self.tx.send(msg).ok()
            }
            Err(e) => self.tx.send(Err(e)).ok(),
        };
    }
}

impl Loader {
    fn reload(&self) {
        match self {
            Loader::Graphics(g) => g.reload(),
            Loader::Compute(g) => g.reload(),
        }
    }
}

struct Handler {
    thread_tx: mpsc::Sender<()>,
    handle: Option<thread::JoinHandle<()>>,
    _watcher: RecommendedWatcher,
}

impl Drop for Handler {
    fn drop(&mut self) {
        self.thread_tx.send(()).ok();
        if let Some(h) = self.handle.take() {
            h.join().ok();
        }
    }
}

fn create_watch(
    src_path: SrcPath,
    frequency: Duration
) -> Result<(Handler, mpsc::Receiver<Result<Message, Error>>), Error> {
    let (notify_tx, notify_rx) = mpsc::channel();
    let (thread_tx, thread_rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher =
        Watcher::new(notify_tx, frequency).map_err(Error::FileWatch)?;

    let (loader, rx) = match src_path {
        SrcPath::Graphics(vert_path, frag_path) => {
            let mut vp = vert_path.clone();
            let mut fp = frag_path.clone();
            vp.pop();
            fp.pop();
            watcher
                .watch(&vp, RecursiveMode::NonRecursive)
                .map_err(Error::FileWatch)?;
            if vp != fp {
                watcher
                    .watch(&fp, RecursiveMode::NonRecursive)
                    .map_err(Error::FileWatch)?;
            }

            let (loader, rx) = GraphicsLoader::create(vert_path, frag_path);
            (Loader::Graphics(loader), rx)
        }
        SrcPath::Compute(compute_path) => {
            let mut cp = compute_path.clone();
            cp.pop();
            watcher
                .watch(&cp, RecursiveMode::NonRecursive)
                .map_err(Error::FileWatch)?;

            let (loader, rx) = ComputeLoader::create(compute_path);
            (Loader::Compute(loader), rx)
        }
    };


    let handle = thread::spawn(move || 'watch_loop: loop {
        if thread_rx.try_recv().is_ok() {
            break 'watch_loop;
        }
        if let Ok(notify::DebouncedEvent::Create(_)) | Ok(notify::DebouncedEvent::Write(_)) =
            notify_rx.recv_timeout(Duration::from_secs(1))
        {
            loader.reload();
        }
    });
    let handle = Some(handle);
    let handler = Handler {
        thread_tx,
        handle,
        _watcher: watcher,
    };
    Ok((handler, rx))
}
