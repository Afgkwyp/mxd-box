//! 念出提醒（「XX 即将结束」）。
//!
//! 用 Windows 自带的语音（SAPI 的 `SpVoice`），直接走 COM 的 FFI —— 和项目里别的系统调用
//! 一样不带额外的 DLL、不起子进程。中文系统自带中文语音（慧慧 / 晓晓之类）；
//! 系统里没装中文语音、或者 COM 起不来，就退回普通提示音，至少让人知道该看一眼了。
//!
//! 网页版能挑语音、上传自己的音频，还有一个在线合成的「曼波」音色；这里先只用系统
//! 默认语音，能调音量和语速。
//!
//! 念是**同步**的，放在一条专门的线程里排队：两个 BUFF 同时到点就一个接一个念，
//! 不会叠在一起，也不会卡住识别循环。

use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

/// 一条要念的话。
struct Utterance {
    text: String,
    /// 0–100
    volume: u8,
    /// -10–10（SAPI 的语速刻度，0 是正常）
    rate: i8,
}

fn queue() -> &'static Mutex<Sender<Utterance>> {
    static QUEUE: OnceLock<Mutex<Sender<Utterance>>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = channel::<Utterance>();
        std::thread::Builder::new()
            .name("buff-speech".into())
            .spawn(move || {
                let voice = win::Voice::new();
                if voice.is_none() {
                    log::warn!("BUFF 提醒：系统语音不可用，改用提示音");
                }
                for utterance in rx {
                    let spoken = voice.as_ref().is_some_and(|voice| {
                        voice.speak(&utterance.text, utterance.volume, utterance.rate)
                    });
                    if !spoken {
                        crate::alert::play_alert_sound();
                    }
                }
            })
            .expect("起语音线程失败");
        Mutex::new(tx)
    })
}

/// 排队念一句话（立刻返回）。
pub fn speak(text: &str, volume: u8, rate: i8) {
    let utterance = Utterance { text: text.to_string(), volume: volume.min(100), rate: rate.clamp(-10, 10) };
    if let Ok(tx) = queue().lock() {
        if tx.send(utterance).is_err() {
            crate::alert::play_alert_sound();
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    /// CLSID_SpVoice {96749377-3391-11D2-9EE3-00C04F797396}
    const CLSID_SP_VOICE: Guid =
        Guid(0x9674_9377, 0x3391, 0x11D2, [0x9E, 0xE3, 0x00, 0xC0, 0x4F, 0x79, 0x73, 0x96]);
    /// IID_ISpVoice {6C44DF74-72B9-4992-A1EC-EF996E0422D4}
    const IID_ISP_VOICE: Guid =
        Guid(0x6C44_DF74, 0x72B9, 0x4992, [0xA1, 0xEC, 0xEF, 0x99, 0x6E, 0x04, 0x22, 0xD4]);
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const CLSCTX_ALL: u32 = 0x17;
    /// SPF_IS_NOT_XML：文字原样念，不当 SSML 解析（BUFF 名字里可能有尖括号）
    const SPF_IS_NOT_XML: u32 = 0x10;

    // ISpVoice 的虚表位置：IUnknown(0–2) + ISpNotifySource(3–9) + ISpEventSource(10–12)
    // 之后才是 ISpVoice 自己的方法。
    const VT_RELEASE: usize = 2;
    const VT_SPEAK: usize = 20;
    const VT_SET_RATE: usize = 28;
    const VT_SET_VOLUME: usize = 30;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            context: u32,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> i32;
    }

    /// 一个 `ISpVoice` 实例。只在创建它的那条线程上用（单线程套间）。
    pub struct Voice(*mut c_void);

    impl Voice {
        pub fn new() -> Option<Self> {
            unsafe {
                // 已经初始化过（S_FALSE）也算成功
                if CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) < 0 {
                    return None;
                }
                let mut ptr: *mut c_void = std::ptr::null_mut();
                let hr = CoCreateInstance(
                    &CLSID_SP_VOICE,
                    std::ptr::null_mut(),
                    CLSCTX_ALL,
                    &IID_ISP_VOICE,
                    &mut ptr,
                );
                (hr >= 0 && !ptr.is_null()).then_some(Self(ptr))
            }
        }

        /// 取虚表第 `index` 项。
        unsafe fn method(&self, index: usize) -> *const c_void {
            let vtable = *(self.0 as *const *const *const c_void);
            *vtable.add(index)
        }

        /// 同步念完一句。返回是否成功。
        pub fn speak(&self, text: &str, volume: u8, rate: i8) -> bool {
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                let set_volume: extern "system" fn(*mut c_void, u16) -> i32 =
                    std::mem::transmute(self.method(VT_SET_VOLUME));
                let set_rate: extern "system" fn(*mut c_void, i32) -> i32 =
                    std::mem::transmute(self.method(VT_SET_RATE));
                let speak: extern "system" fn(*mut c_void, *const u16, u32, *mut u32) -> i32 =
                    std::mem::transmute(self.method(VT_SPEAK));
                set_volume(self.0, volume as u16);
                set_rate(self.0, rate as i32);
                speak(self.0, wide.as_ptr(), SPF_IS_NOT_XML, std::ptr::null_mut()) >= 0
            }
        }
    }

    impl Drop for Voice {
        fn drop(&mut self) {
            unsafe {
                let release: extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(self.method(VT_RELEASE));
                release(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod win {
    pub struct Voice;

    impl Voice {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn speak(&self, _text: &str, _volume: u8, _rate: i8) -> bool {
            false
        }
    }
}
