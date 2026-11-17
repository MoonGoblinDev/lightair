//! Lightair Virtual Camera on Windows 11: a Media Foundation camera source, loaded by Windows'
//! Frame Server when an app opens the camera Lightair registers with `MFCreateVirtualCamera`.
//! It shows what Lightair writes into shared memory (protocol.rs), or black while Lightair
//! isn't sending.
//!
//! The pieces, as Windows asks for them: a class factory makes an activator (`IMFActivate`),
//! which makes the source (`IMFMediaSourceEx`), which has one video stream (`IMFMediaStream2`)
//! that answers each sample request with the latest frame, paced to 30 a second.
#![cfg(windows)]
#![allow(non_snake_case)]

pub mod protocol;

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, Instant};

use protocol as p;
use windows::Win32::Foundation::*;
use windows::Win32::Media::KernelStreaming::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Variant::VT_I8;
use windows::core::*;

const CLSID: GUID = GUID::from_u128(p::CLSID);
const FRAME_100NS: i64 = 10_000_000 / p::FPS as i64;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ---- the DLL's exports ----

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(clsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() || clsid.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = std::ptr::null_mut() };
    if unsafe { *clsid } != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    unsafe { factory.query(riid, ppv) }
}

/// The activators, sources and streams alive: the DLL can go (and a newer one be installed)
/// once there are none.
static ALIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts an object alive while it is.
struct Alive;

impl Alive {
    fn new() -> Self {
        ALIVE.fetch_add(1, Ordering::AcqRel);
        Alive
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        ALIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if ALIVE.load(Ordering::Acquire) == 0 { S_OK } else { S_FALSE }
}

/// The DLL's own path (what the registry points at).
fn own_path() -> Option<String> {
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(DllGetClassObject as *const u16),
            &mut module,
        )
        .ok()?;
    }
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

fn class_key() -> String {
    format!(r"SOFTWARE\Classes\CLSID\{}", p::CLSID_STRING)
}

fn set_string(key: HKEY, name: Option<&str>, value: &str) -> WIN32_ERROR {
    let v = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
    let name = name.map(wide);
    unsafe { RegSetValueExW(key, name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())), None, REG_SZ, Some(bytes)) }
}

/// `regsvr32 lightair_camera.dll` (as administrator): the class in HKLM, where the Frame
/// Server service looks.
#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    let Some(path) = own_path() else { return E_FAIL };
    let make = |sub: &str| -> std::result::Result<HKEY, WIN32_ERROR> {
        let mut key = HKEY::default();
        let name = wide(sub);
        let r = unsafe {
            RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(name.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
        };
        if r == ERROR_SUCCESS { Ok(key) } else { Err(r) }
    };
    let class = match make(&class_key()) {
        Ok(k) => k,
        Err(e) => return e.to_hresult(),
    };
    let r = set_string(class, None, p::NAME);
    unsafe {
        let _ = RegCloseKey(class);
    }
    if r != ERROR_SUCCESS {
        return r.to_hresult();
    }
    let server = match make(&format!(r"{}\InprocServer32", class_key())) {
        Ok(k) => k,
        Err(e) => return e.to_hresult(),
    };
    let mut r = set_string(server, None, &path);
    if r == ERROR_SUCCESS {
        r = set_string(server, Some("ThreadingModel"), "Both");
    }
    unsafe {
        let _ = RegCloseKey(server);
    }
    r.to_hresult()
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let name = wide(&class_key());
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(name.as_ptr())) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { S_OK } else { r.to_hresult() }
}

// ---- class factory ----

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if outer.as_ref().is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activate: IMFActivate = Activate::new()?.into();
        unsafe { activate.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

// ---- activator ----

#[implement(IMFActivate)]
struct Activate {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSourceEx>>,
    _alive: Alive,
}

impl Activate {
    fn new() -> Result<Self> {
        Ok(Self { attrs: attributes(4)?, source: Mutex::new(None), _alive: Alive::new() })
    }
}

fn attributes(n: u32) -> Result<IMFAttributes> {
    let mut a = None;
    unsafe { MFCreateAttributes(&mut a, n)? };
    a.ok_or_else(|| E_OUTOFMEMORY.into())
}

fn raw<T: Interface>(r: &Ref<T>) -> *mut c_void {
    r.as_ref().map_or(std::ptr::null_mut(), |i| i.as_raw())
}

/// Calls the inner attribute store's own method (the activator's attributes are its).
macro_rules! inner {
    ($self:ident . $m:ident ( $($a:expr),* )) => {
        unsafe { (Interface::vtable(&$self.attrs).$m)(Interface::as_raw(&$self.attrs), $($a),*).ok() }
    };
}

impl IMFAttributes_Impl for Activate_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> Result<()> {
        inner!(self.GetItem(k, v))
    }
    fn GetItemType(&self, k: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        let mut t = MF_ATTRIBUTE_TYPE::default();
        inner!(self.GetItemType(k, &mut t))?;
        Ok(t)
    }
    fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<BOOL> {
        let mut b = BOOL::default();
        inner!(self.CompareItem(k, v, &mut b))?;
        Ok(b)
    }
    fn Compare(&self, theirs: Ref<IMFAttributes>, t: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
        let mut b = BOOL::default();
        inner!(self.Compare(raw(&theirs), t, &mut b))?;
        Ok(b)
    }
    fn GetUINT32(&self, k: *const GUID) -> Result<u32> {
        let mut v = 0;
        inner!(self.GetUINT32(k, &mut v))?;
        Ok(v)
    }
    fn GetUINT64(&self, k: *const GUID) -> Result<u64> {
        let mut v = 0;
        inner!(self.GetUINT64(k, &mut v))?;
        Ok(v)
    }
    fn GetDouble(&self, k: *const GUID) -> Result<f64> {
        let mut v = 0.0;
        inner!(self.GetDouble(k, &mut v))?;
        Ok(v)
    }
    fn GetGUID(&self, k: *const GUID) -> Result<GUID> {
        let mut v = GUID::zeroed();
        inner!(self.GetGUID(k, &mut v))?;
        Ok(v)
    }
    fn GetStringLength(&self, k: *const GUID) -> Result<u32> {
        let mut v = 0;
        inner!(self.GetStringLength(k, &mut v))?;
        Ok(v)
    }
    fn GetString(&self, k: *const GUID, s: PWSTR, n: u32, len: *mut u32) -> Result<()> {
        inner!(self.GetString(k, s, n, len))
    }
    fn GetAllocatedString(&self, k: *const GUID, s: *mut PWSTR, len: *mut u32) -> Result<()> {
        inner!(self.GetAllocatedString(k, s, len))
    }
    fn GetBlobSize(&self, k: *const GUID) -> Result<u32> {
        let mut v = 0;
        inner!(self.GetBlobSize(k, &mut v))?;
        Ok(v)
    }
    fn GetBlob(&self, k: *const GUID, b: *mut u8, n: u32, len: *mut u32) -> Result<()> {
        inner!(self.GetBlob(k, b, n, len))
    }
    fn GetAllocatedBlob(&self, k: *const GUID, b: *mut *mut u8, len: *mut u32) -> Result<()> {
        inner!(self.GetAllocatedBlob(k, b, len))
    }
    fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        inner!(self.GetUnknown(k, riid, ppv))
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<()> {
        inner!(self.SetItem(k, v))
    }
    fn DeleteItem(&self, k: *const GUID) -> Result<()> {
        inner!(self.DeleteItem(k))
    }
    fn DeleteAllItems(&self) -> Result<()> {
        inner!(self.DeleteAllItems())
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> Result<()> {
        inner!(self.SetUINT32(k, v))
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> Result<()> {
        inner!(self.SetUINT64(k, v))
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> Result<()> {
        inner!(self.SetDouble(k, v))
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> Result<()> {
        inner!(self.SetGUID(k, v))
    }
    fn SetString(&self, k: *const GUID, v: &PCWSTR) -> Result<()> {
        inner!(self.SetString(k, *v))
    }
    fn SetBlob(&self, k: *const GUID, b: *const u8, n: u32) -> Result<()> {
        inner!(self.SetBlob(k, b, n))
    }
    fn SetUnknown(&self, k: *const GUID, u: Ref<IUnknown>) -> Result<()> {
        inner!(self.SetUnknown(k, raw(&u)))
    }
    fn LockStore(&self) -> Result<()> {
        inner!(self.LockStore())
    }
    fn UnlockStore(&self) -> Result<()> {
        inner!(self.UnlockStore())
    }
    fn GetCount(&self) -> Result<u32> {
        let mut v = 0;
        inner!(self.GetCount(&mut v))?;
        Ok(v)
    }
    fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> Result<()> {
        inner!(self.GetItemByIndex(i, k, v))
    }
    fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> Result<()> {
        inner!(self.CopyAllItems(raw(&dest)))
    }
}

impl IMFActivate_Impl for Activate_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        let mut slot = self.source.lock().unwrap();
        if slot.is_none() {
            *slot = Some(new_source()?);
        }
        unsafe { slot.as_ref().unwrap().query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        if let Some(s) = self.source.lock().unwrap().take() {
            let _ = unsafe { s.Shutdown() };
        }
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        self.source.lock().unwrap().take();
        Ok(())
    }
}

// ---- the source ----

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
struct Source {
    st: Mutex<SourceState>,
    _alive: Alive,
}

struct SourceState {
    queue: Option<IMFMediaEventQueue>,
    pd: IMFPresentationDescriptor,
    attrs: IMFAttributes,
    stream: Option<IMFMediaStream2>,
    started: bool,
}

fn media_type() -> Result<IMFMediaType> {
    let t = unsafe { MFCreateMediaType()? };
    unsafe {
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((p::WIDTH as u64) << 32) | p::HEIGHT as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((p::FPS as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, p::FRAME_SIZE as u32)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, p::WIDTH)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, p::FRAME_SIZE as u32 * 8 * p::FPS)?;
        // the colours as Lightair writes them (protocol.rs)
        t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
    }
    Ok(t)
}

fn new_source() -> Result<IMFMediaSourceEx> {
    let attrs = attributes(4)?;
    unsafe {
        attrs.SetUINT32(&MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1)?;
        attrs.SetGUID(&MFT_TRANSFORM_CLSID_Attribute, &CLSID)?;
    }
    let types = [Some(media_type()?)];
    let sd = unsafe { MFCreateStreamDescriptor(0, &types)? };
    unsafe { sd.GetMediaTypeHandler()?.SetCurrentMediaType(types[0].as_ref().unwrap())? };
    let pd = unsafe { MFCreatePresentationDescriptor(Some(&[Some(sd.clone())]))? };
    unsafe { pd.SelectStream(0)? };
    let source: IMFMediaSourceEx =
        Source {
            st: Mutex::new(SourceState { queue: Some(unsafe { MFCreateEventQueue()? }), pd, attrs, stream: None, started: false }),
            _alive: Alive::new(),
        }
        .into();
    let stream: IMFMediaStream2 = Stream::new(source.cast()?, sd)?.into();
    unsafe { source.as_impl() }.st.lock().unwrap().stream = Some(stream);
    Ok(source)
}

fn not_found() -> Error {
    HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into()
}

fn time_variant(t: i64) -> PROPVARIANT {
    let mut v = PROPVARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = t;
    }
    v
}

impl Source_Impl {
    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.st.lock().unwrap().queue.clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // (not under the lock: this waits)
        unsafe { self.queue()?.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        unsafe { self.queue()?.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue()?.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue()?.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        self.queue()?;
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        let st = self.st.lock().unwrap();
        if st.queue.is_none() {
            return Err(MF_E_SHUTDOWN.into());
        }
        unsafe { st.pd.Clone() }
    }

    fn Start(&self, pd: Ref<IMFPresentationDescriptor>, _format: *const GUID, _start: *const PROPVARIANT) -> Result<()> {
        let pd = pd.ok()?;
        let (queue, stream, was) = {
            let mut st = self.st.lock().unwrap();
            let queue = st.queue.clone().ok_or(Error::from(MF_E_SHUTDOWN))?;
            let was = std::mem::replace(&mut st.started, true);
            (queue, st.stream.clone().ok_or(Error::from(MF_E_SHUTDOWN))?, was)
        };
        let time = time_variant(unsafe { MFGetSystemTime() });
        let mut selected = BOOL::default();
        let mut sd = None;
        unsafe { pd.GetStreamDescriptorByIndex(0, &mut selected, &mut sd)? };
        let s = unsafe { stream.as_impl() };
        if selected.as_bool() {
            let sd = sd.ok_or(Error::from(E_UNEXPECTED))?;
            let ty = unsafe { sd.GetMediaTypeHandler()?.GetCurrentMediaType()? };
            let event = if was { MEUpdatedStream } else { MENewStream };
            unsafe { queue.QueueEventParamUnk(event.0 as u32, &GUID::zeroed(), S_OK, &stream)? };
            s.start(&ty, &time)?;
        } else {
            s.stop();
        }
        unsafe { queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), S_OK, &time) }
    }

    fn Stop(&self) -> Result<()> {
        let (queue, stream) = {
            let mut st = self.st.lock().unwrap();
            st.started = false;
            (st.queue.clone().ok_or(Error::from(MF_E_SHUTDOWN))?, st.stream.clone())
        };
        if let Some(s) = &stream {
            unsafe { s.as_impl() }.stop();
        }
        let time = time_variant(unsafe { MFGetSystemTime() });
        unsafe { queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), S_OK, &time) }
    }

    fn Pause(&self) -> Result<()> {
        // a live camera doesn't pause
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        let (queue, stream) = {
            let mut st = self.st.lock().unwrap();
            (st.queue.take(), st.stream.take())
        };
        if let Some(s) = stream {
            unsafe { s.as_impl() }.shutdown();
        }
        match queue {
            Some(q) => unsafe { q.Shutdown() },
            None => Err(MF_E_SHUTDOWN.into()),
        }
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.st.lock().unwrap().attrs.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        let st = self.st.lock().unwrap();
        match (&st.stream, id) {
            (Some(s), 0) => Ok(unsafe { s.as_impl() }.attrs.clone()),
            (None, _) => Err(MF_E_SHUTDOWN.into()),
            _ => Err(E_INVALIDARG.into()),
        }
    }

    fn SetD3DManager(&self, _: Ref<IUnknown>) -> Result<()> {
        // frames are made in system memory
        Ok(())
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _: *const GUID, _: *const GUID, _: *mut *mut c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for Source_Impl {
    fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
    fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
    fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
}

// ---- the stream ----

#[implement(IMFMediaStream2, IKsControl)]
struct Stream {
    attrs: IMFAttributes,
    st: Mutex<StreamState>,
    _alive: Alive,
}

struct StreamState {
    queue: Option<IMFMediaEventQueue>,
    source: Option<IMFMediaSource>,
    sd: IMFStreamDescriptor,
    state: MF_STREAM_STATE,
    allocator: Option<IMFVideoSampleAllocatorEx>,
    shared: Option<Shared>,
    /// When the next frame is due.
    next: Option<Instant>,
}

impl Stream {
    fn new(source: IMFMediaSource, sd: IMFStreamDescriptor) -> Result<Self> {
        let attrs = attributes(4)?;
        unsafe {
            attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;
        }
        Ok(Self {
            attrs,
            _alive: Alive::new(),
            st: Mutex::new(StreamState {
                queue: Some(unsafe { MFCreateEventQueue()? }),
                source: Some(source),
                sd,
                state: MF_STREAM_STATE_STOPPED,
                allocator: None,
                shared: None,
                next: None,
            }),
        })
    }

    fn start(&self, ty: &IMFMediaType, time: &PROPVARIANT) -> Result<()> {
        let queue = {
            let mut st = self.st.lock().unwrap();
            let queue = st.queue.clone().ok_or(Error::from(MF_E_SHUTDOWN))?;
            if st.allocator.is_none() {
                let mut a: *mut c_void = std::ptr::null_mut();
                unsafe { MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut a)? };
                let a = unsafe { IMFVideoSampleAllocatorEx::from_raw(a) };
                unsafe { a.InitializeSampleAllocator(10, ty)? };
                st.allocator = Some(a);
            }
            if st.shared.is_none() {
                st.shared = Shared::make();
            }
            st.state = MF_STREAM_STATE_RUNNING;
            st.next = None;
            queue
        };
        unsafe { queue.QueueEventParamVar(MEStreamStarted.0 as u32, &GUID::zeroed(), S_OK, time) }
    }

    fn stop(&self) {
        let queue = {
            let mut st = self.st.lock().unwrap();
            let was = st.state;
            st.state = MF_STREAM_STATE_STOPPED;
            if was == MF_STREAM_STATE_STOPPED { None } else { st.queue.clone() }
        };
        if let Some(q) = queue {
            let time = time_variant(unsafe { MFGetSystemTime() });
            let _ = unsafe { q.QueueEventParamVar(MEStreamStopped.0 as u32, &GUID::zeroed(), S_OK, &time) };
        }
    }

    fn shutdown(&self) {
        let mut st = self.st.lock().unwrap();
        st.state = MF_STREAM_STATE_STOPPED;
        if let Some(q) = st.queue.take() {
            let _ = unsafe { q.Shutdown() };
        }
        if let Some(a) = st.allocator.take() {
            let _ = unsafe { a.UninitializeSampleAllocator() };
        }
        // (the source holds the stream: let go of it, no cycle)
        st.source = None;
        st.shared = None;
    }
}

impl Stream_Impl {
    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.st.lock().unwrap().queue.clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue()?.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        unsafe { self.queue()?.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue()?.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue()?.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.st.lock().unwrap().source.clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        let st = self.st.lock().unwrap();
        if st.queue.is_none() {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(st.sd.clone())
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        let (queue, sample) = {
            let mut st = self.st.lock().unwrap();
            let queue = st.queue.clone().ok_or(Error::from(MF_E_SHUTDOWN))?;
            if st.state != MF_STREAM_STATE_RUNNING {
                return Err(MF_E_INVALIDREQUEST.into());
            }
            // paced: a frame each 1/30 s (the Frame Server asks again as soon as it has one)
            let now = Instant::now();
            let period = Duration::from_nanos(1_000_000_000 / p::FPS as u64);
            let due = st.next.unwrap_or(now);
            if due > now {
                std::thread::sleep(due - now);
            }
            let after = Instant::now();
            st.next = Some(if after > due + period { after + period } else { due + period });
            let allocator = st.allocator.clone().ok_or(Error::from(MF_E_NOT_INITIALIZED))?;
            let sample = unsafe { allocator.AllocateSample()? };
            let buffer = unsafe { sample.GetBufferByIndex(0)? };
            fill(&buffer, st.shared.as_ref())?;
            (queue, sample)
        };
        unsafe {
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_100NS)?;
            if let Some(t) = token.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, t)?;
            }
            queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), S_OK, &sample)
        }
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        let running = self.st.lock().unwrap().state == MF_STREAM_STATE_RUNNING;
        match value {
            MF_STREAM_STATE_STOPPED if running => self.stop(),
            v => self.st.lock().unwrap().state = v,
        }
        Ok(())
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.st.lock().unwrap().state)
    }
}

impl IKsControl_Impl for Stream_Impl {
    fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
    fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
    fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_found())
    }
}

// ---- the frame ----

/// Writes the latest frame (or black) into `buffer`.
fn fill(buffer: &IMFMediaBuffer, shared: Option<&Shared>) -> Result<()> {
    let (w, h) = (p::WIDTH as usize, p::HEIGHT as usize);
    let copy = |dst: *mut u8, pitch: isize| {
        let row = |plane_row: usize| unsafe { dst.offset(pitch * plane_row as isize) };
        if shared.is_some_and(|s| s.read(|src| unsafe { copy_nv12(src, w, h, &row) })) {
            return;
        }
        // black: Y 16, UV 128
        for y in 0..h {
            unsafe { std::ptr::write_bytes(row(y), 16, w) };
        }
        for y in 0..h / 2 {
            unsafe { std::ptr::write_bytes(row(h + y), 128, w) };
        }
    };
    if let Ok(b2) = buffer.cast::<IMF2DBuffer2>() {
        let (mut scan0, mut pitch, mut start, mut len) = (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
        unsafe { b2.Lock2DSize(MF2DBuffer_LockFlags_Write, &mut scan0, &mut pitch, &mut start, &mut len)? };
        copy(scan0, pitch as isize);
        unsafe { b2.Unlock2D()? };
    } else {
        let mut data = std::ptr::null_mut();
        unsafe { buffer.Lock(&mut data, None, None)? };
        copy(data, w as isize);
        unsafe {
            buffer.Unlock()?;
            buffer.SetCurrentLength(p::FRAME_SIZE as u32)?;
        }
    }
    Ok(())
}

/// Copies a packed NV12 frame (`src`) into rows given by `row` (the UV plane's rows follow Y's).
unsafe fn copy_nv12(src: *const u8, w: usize, h: usize, row: &dyn Fn(usize) -> *mut u8) {
    for y in 0..h + h / 2 {
        unsafe { std::ptr::copy_nonoverlapping(src.add(y * w), row(y), w) };
    }
}

/// The shared memory Lightair writes frames into.
struct Shared {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for Shared {}

impl Shared {
    /// Made (or opened, if Lightair's camera was opened before) as `Global\`, open to the
    /// signed-in user; `Local\` where global memory can't be made.
    fn make() -> Option<Self> {
        // SYSTEM and Local Service (the Frame Server): all; signed-in users: read and write
        let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GRGW;;;AU)");
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let have_sd = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }.is_ok();
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let mut made = None;
        for name in [p::MEMORY_GLOBAL, p::MEMORY_LOCAL] {
            let n = wide(name);
            if let Ok(h) = unsafe {
                CreateFileMappingW(INVALID_HANDLE_VALUE, have_sd.then_some(&sa as *const _), PAGE_READWRITE, 0, p::SIZE as u32, PCWSTR(n.as_ptr()))
            } {
                made = Some(h);
                break;
            }
        }
        if have_sd {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
        }
        let handle = made?;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, p::SIZE) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }
        let header = view.Value as *mut p::Header;
        unsafe {
            (*header).magic = p::MAGIC;
            (*header).version = p::VERSION;
            (*header).width = p::WIDTH;
            (*header).height = p::HEIGHT;
        }
        Some(Self { handle, view })
    }

    fn header(&self) -> &p::Header {
        unsafe { &*(self.view.Value as *const p::Header) }
    }

    /// Calls `with` on a whole, recent frame; false if there's none (Lightair isn't sending).
    fn read(&self, with: impl Fn(*const u8)) -> bool {
        let h = self.header();
        let now = unsafe { GetTickCount64() };
        h.read_ms.store(now, Ordering::Relaxed);
        if now.saturating_sub(h.written_ms.load(Ordering::Acquire)) > p::STALE_MS {
            return false;
        }
        let frame = unsafe { (self.view.Value as *const u8).add(p::HEADER_SIZE) };
        for _ in 0..4 {
            let before = h.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            with(frame);
            fence(Ordering::Acquire);
            if h.seq.load(Ordering::Relaxed) == before {
                return true;
            }
        }
        // (torn each time: what was copied is still nearly the frame)
        true
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Lightair's output does (crates/lightair-engine/src/vcam.rs), in short: one flat frame.
    fn write_frame(y: u8, uv: u8) -> bool {
        let n = wide(p::MEMORY_LOCAL);
        let global = wide(p::MEMORY_GLOBAL);
        // (read and write, as the engine asks: what the camera's memory grants users)
        let rw = FILE_MAP_READ | FILE_MAP_WRITE;
        let Ok(h) = (unsafe { OpenFileMappingW(rw.0, false, PCWSTR(global.as_ptr())) })
            .or_else(|_| unsafe { OpenFileMappingW(rw.0, false, PCWSTR(n.as_ptr())) })
        else {
            return false;
        };
        let view = unsafe { MapViewOfFile(h, rw, 0, 0, p::SIZE) };
        let header = unsafe { &*(view.Value as *const p::Header) };
        assert_eq!((header.magic, header.width, header.height), (p::MAGIC, p::WIDTH, p::HEIGHT));
        let frame = unsafe { (view.Value as *mut u8).add(p::HEADER_SIZE) };
        let luma = (p::WIDTH * p::HEIGHT) as usize;
        header.seq.fetch_add(1, Ordering::AcqRel);
        unsafe {
            std::ptr::write_bytes(frame, y, luma);
            std::ptr::write_bytes(frame.add(luma), uv, luma / 2);
        }
        header.seq.fetch_add(1, Ordering::Release);
        header.written_ms.store(unsafe { GetTickCount64() }, Ordering::Release);
        unsafe {
            let _ = UnmapViewOfFile(view);
            let _ = CloseHandle(h);
        }
        true
    }

    fn first_pixels(sample: &IMFSample) -> (u8, u8) {
        let b = unsafe { sample.ConvertToContiguousBuffer().unwrap() };
        let mut data = std::ptr::null_mut();
        let mut len = 0;
        unsafe { b.Lock(&mut data, None, Some(&mut len)).unwrap() };
        assert!(len as usize >= p::FRAME_SIZE, "{len}");
        let px = unsafe { (*data, *data.add((p::WIDTH * p::HEIGHT) as usize)) };
        unsafe { b.Unlock().unwrap() };
        px
    }

    /// The camera, as an app sees it through Media Foundation (in this process, not the Frame
    /// Server): black until Lightair writes, then Lightair's frame, at 30 a second.
    #[test]
    fn an_app_reads_what_lightair_writes() {
        unsafe { MFStartup(0x0002_0070, MFSTARTUP_FULL).unwrap() };
        let factory: IClassFactory = Factory.into();
        let activate: IMFActivate = unsafe { factory.CreateInstance(None).unwrap() };
        let source: IMFMediaSource = unsafe { activate.ActivateObject().unwrap() };
        let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source, None).unwrap() };
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let ty = unsafe { reader.GetCurrentMediaType(stream).unwrap() };
        assert_eq!(unsafe { ty.GetGUID(&MF_MT_SUBTYPE).unwrap() }, MFVideoFormat_NV12);
        assert_eq!(unsafe { ty.GetUINT64(&MF_MT_FRAME_SIZE).unwrap() }, ((p::WIDTH as u64) << 32) | p::HEIGHT as u64);
        let read = || {
            let mut sample = None;
            let mut flags = 0;
            unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).unwrap() };
            sample.unwrap_or_else(|| panic!("no sample (flags {flags:#x})"))
        };
        // nothing written yet: black
        assert_eq!(first_pixels(&read()), (16, 128));
        assert!(write_frame(200, 60), "the camera's memory");
        assert_eq!(first_pixels(&read()), (200, 60));
        // paced: 15 frames take about half a second
        let t = Instant::now();
        for _ in 0..15 {
            read();
        }
        let ms = t.elapsed().as_millis();
        eprintln!("15 frames in {ms} ms");
        assert!((400..800).contains(&ms), "{ms} ms");
        unsafe {
            source.Shutdown().unwrap();
            activate.ShutdownObject().unwrap();
        }
    }
}

/// Adds the camera for this session (registered for this user only, HKCU) and has ffmpeg open
/// it: whether the Frame Server takes a per-user registration. Needs the DLL built
/// (`cargo build -p lightair-camera-win`). Run by hand.
#[cfg(test)]
mod frame_server {
    use super::*;

    #[test]
    #[ignore]
    fn ffmpeg_opens_the_camera() {
        let dll = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\target\debug\lightair_camera.dll");
        let dll = dll.canonicalize().expect("the DLL, built");
        let key = format!(r"HKCU\Software\Classes\CLSID\{}\InprocServer32", p::CLSID_STRING);
        let reg = |args: &[&str]| std::process::Command::new("reg").args(args).output().unwrap().status.success();
        assert!(reg(&["add", &key, "/ve", "/d", dll.to_str().unwrap(), "/f"]));
        assert!(reg(&["add", &key, "/v", "ThreadingModel", "/d", "Both", "/f"]));
        unsafe { MFStartup(0x0002_0070, MFSTARTUP_FULL).unwrap() };
        let camera = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &HSTRING::from(p::NAME),
                &HSTRING::from(p::CLSID_STRING),
                None,
            )
        };
        let result = camera.as_ref().map_err(|e| e.clone()).and_then(|c| unsafe { c.Start(None) });
        eprintln!("added and started: {result:?}");
        if result.is_ok() {
            let out = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-f", "dshow", "-i", &format!("video={}", p::NAME), "-frames:v", "5", "-f", "null", "-"])
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            eprintln!("ffmpeg: {}\n{}", out.status, err.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        if let Ok(c) = &camera {
            let _ = unsafe { c.Remove() };
        }
        let _ = reg(&["delete", &format!(r"HKCU\Software\Classes\CLSID\{}", p::CLSID_STRING), "/f"]);
    }
}
