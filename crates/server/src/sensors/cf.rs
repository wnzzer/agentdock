//! The Core Foundation and IOKit calls the macOS readers share, and an owned
//! handle that releases what they create.
use std::ffi::{CStr, c_char, c_void};

pub(super) type CFTypeRef = *const c_void;
pub(super) type CFIndex = isize;

const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCFAllocatorDefault: CFTypeRef;
    pub(super) fn CFRelease(object: CFTypeRef);
    pub(super) fn CFGetTypeID(object: CFTypeRef) -> usize;
    pub(super) fn CFDataGetTypeID() -> usize;
    pub(super) fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    pub(super) fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    pub(super) fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    pub(super) fn CFStringGetCString(
        text: CFTypeRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> bool;
    pub(super) fn CFDictionaryGetCount(dictionary: CFTypeRef) -> CFIndex;
    pub(super) fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub(super) fn CFDictionaryCreateMutableCopy(
        allocator: CFTypeRef,
        capacity: CFIndex,
        dictionary: CFTypeRef,
    ) -> CFTypeRef;
    pub(super) fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    pub(super) fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
    pub(super) fn CFDictionaryGetTypeID() -> usize;
    pub(super) fn CFNumberGetTypeID() -> usize;
    pub(super) fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, value: *mut c_void) -> bool;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub(super) fn IOServiceNameMatching(name: *const c_char) -> *mut c_void;
    pub(super) fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    pub(super) fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
    pub(super) fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
    pub(super) fn IOObjectRelease(object: u32) -> i32;
}

/// A Core Foundation object this code created, released when dropped.
pub(super) struct Owned(pub(super) CFTypeRef);

impl Owned {
    pub(super) fn new(object: CFTypeRef) -> Option<Self> {
        (!object.is_null()).then_some(Self(object))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: only created from a Create/Copy call, so this holds a reference.
        unsafe { CFRelease(self.0) }
    }
}

pub(super) fn cf_string(text: &CStr) -> Option<Owned> {
    // SAFETY: a NUL-terminated string in, an owned CFString or null out.
    Owned::new(unsafe { CFStringCreateWithCString(kCFAllocatorDefault, text.as_ptr(), UTF8) })
}

pub(super) fn rust_string(text: CFTypeRef) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 128];
    // SAFETY: the buffer's length is passed, and the call NUL-terminates it on success.
    unsafe { CFStringGetCString(text, buffer.as_mut_ptr(), buffer.len() as CFIndex, UTF8) }.then(
        || {
            unsafe { CStr::from_ptr(buffer.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        },
    )
}

/// kCFNumberDoubleType: any CFNumber reads out as a double.
const DOUBLE: CFIndex = 13;

/// A CFNumber's value; none when the object is not a number.
pub(super) fn number(value: CFTypeRef) -> Option<f64> {
    // SAFETY: checked to be a CFNumber before it is read into a local double.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut number = 0f64;
        CFNumberGetValue(value, DOUBLE, (&mut number as *mut f64).cast())
            .then_some(number)
            .filter(|number: &f64| number.is_finite())
    }
}

/// A number in a CF dictionary, by key; none when absent or not a number.
pub(super) fn dictionary_number(dictionary: CFTypeRef, key: &CStr) -> Option<f64> {
    let key = cf_string(key)?;
    // SAFETY: a live dictionary and key; the value is borrowed for the read.
    number(unsafe { CFDictionaryGetValue(dictionary, key.0) })
}

/// One property of the first IOKit service of `class`, owned.
pub(super) fn service_property(class: &CStr, key: &CStr) -> Option<Owned> {
    let key = cf_string(key)?;
    // SAFETY: IOKit lookups; the matching dictionary is consumed by the lookup
    // and the service released once its property is copied.
    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(class.as_ptr()));
        if service == 0 {
            return None;
        }
        let value = IORegistryEntryCreateCFProperty(service, key.0, kCFAllocatorDefault, 0);
        IOObjectRelease(service);
        Owned::new(value)
    }
}

/// Whether a CF object is a dictionary.
pub(super) fn is_dictionary(object: CFTypeRef) -> bool {
    // SAFETY: a live CF object.
    !object.is_null() && unsafe { CFGetTypeID(object) == CFDictionaryGetTypeID() }
}
