/* Copyright (C) 2018 Olivier Goffart <ogoffart@woboq.com>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES
OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
*/

/*! This crate implements binding to the Qt API which allow to use QML from a rust application.

    # Example:

    ```
    extern crate qmetaobject;
    use qmetaobject::*;
    #[macro_use] extern crate cstr;

    // The `QObject` custom derive macro allows to expose a class to Qt and QML
    #[derive(QObject,Default)]
    struct Greeter {
        // Specify the base class with the qt_base_class macro
        base : qt_base_class!(trait QObject),
        // Decalare `name` as a property usable from Qt
        name : qt_property!(QString; NOTIFY name_changed),
        // Declare a signal
        name_changed : qt_signal!(),
        // And even a slot
        compute_greetings : qt_method!(fn compute_greetings(&self, verb : String) -> QString {
            return (verb + " " + &self.name.to_string()).into()
        })
    }

    fn main() {
        // Register the `Greeter` struct to QML
        qml_register_type::<Greeter>(cstr!("Greeter"), 1, 0, cstr!("Greeter"));
        // Create a QML engine from rust
        let mut engine = QmlEngine::new();
    # return; // We can't create a window in the CI
        // (Here the QML code is inline, but one can also load from a file)
        engine.load_data(r#"
            import QtQuick 2.6;
            import QtQuick.Window 2.0;
            import Greeter 1.0  // import our Rust classes
            Window {
                visible: true;
                Greeter { //  Instentiate the rust struct
                    id: greeter;
                    name: 'World'; // set a property
                }
                Text {
                    anchors.centerIn: parent;
                    // Call a method
                    text: greeter.compute_greetings('hello');
                }
            }"#.into());
        engine.exec();
    }
    ```

    # Basic types

    The re-exported module [qttypes](qttypes/index.html) contains binding to the most usefull
    basic types such as [QString](qttypes/struct.QString.html),
    [QVariant](qttypes/struct.QVariant.html), ...

    You can also simply use rust type `String`, but using QString might avoid unecessary
    conversions in some case.

    # Meta type

    In order to be able to use a type in a signal or method parameter, or as a property type,
    the type need to implement the [QMetaType](qmetatype/trait.QMetaType.html) trait.
    All the method are provided so you can just implement the QMetaType like this:

    ```rust
    # use qmetaobject::QMetaType;
    #[derive(Default, Clone)]
    struct MyPoint(u32, u32);
    impl QMetaType for MyPoint {};
    ```

    With that it is also possible to put the type in a  [QVariant](qttypes/struct.QVariant.html)

    # Object pinning

    Once an object that derives from QObject is exposed to C++, it needs to be pinned, and cannot
    be moved in memory.
    Also, since the Qt code can be re-entrant, the object must be placed in a RefCell.
    The [QObjectPinned](struct.QObjectPinned.html) object is used to enforce the pinning.

    If you want to keep pointer to reference, tou can use [QPointer](struct.QPointer.html).

    # Threading

    The QML engine only runs in a single thread. And probably all the QObject needs to be living
    in the Qt thread. But you can use the [queued_callback](fn.queued_callback.html) function to
    create callback that can be called from any thread and are going to run in the Qt thread.

    This can be done like so:

    ```
    # extern crate qmetaobject;
    # use qmetaobject::*;
    #[derive(QObject,Default)]
    struct MyAsyncObject {
        base : qt_base_class!(trait QObject),
        result : qt_property!(QString; NOTIFY result_changed),
        result_changed : qt_signal!(),
        recompute_result : qt_method!(fn recompute_result(&self, name : String) {
            let qptr = QPointer::from(&*self);
            let set_value = qmetaobject::queued_callback(move |val : QString| {
                qptr.as_pinned().map(|self_| {
                    self_.borrow_mut().result = val;
                    self_.borrow().result_changed();
                });
            });
            std::thread::spawn(move || {
                // do stuff asynchroniously ...
                let r = QString::from("Hello ".to_owned() + &name);
                set_value(r);
            }).join();
        })
    }
    # let obj = std::cell::RefCell::new(MyAsyncObject::default());
    # let mut engine = QmlEngine::new();
    # unsafe { qmetaobject::connect(
    #     QObject::cpp_construct(&obj),
    #     obj.borrow().result_changed.to_cpp_representation(&*obj.borrow()),
    #     || engine.quit()
    # ) };
    # obj.borrow().recompute_result("World".into());
    # engine.exec();
    # assert_eq!(obj.borrow().result, QString::from("Hello World"));
    ```

*/

#![recursion_limit = "10240"]
#![cfg_attr(feature = "cargo-clippy", allow(clippy::needless_pass_by_value))] // Too many of that for qt types. (FIXME)
#![cfg_attr(feature = "cargo-clippy", allow(clippy::cognitive_complexity))]

#[macro_use]
extern crate cpp;

#[allow(unused_imports)]
#[macro_use]
extern crate qmetaobject_impl;
#[doc(hidden)]
pub use qmetaobject_impl::*;

/* In order to be able to use the lazy_static macro from the QObject custom derive, we re-export
   it under a new name qmetaobject_lazy_static */
extern crate lazy_static;
#[allow(unused_imports)]
#[doc(hidden)]
pub use lazy_static::*;
#[doc(hidden)]
#[macro_export]
macro_rules! qmetaobject_lazy_static { ($($t:tt)*) => { lazy_static!($($t)*) } }

//#[macro_use]
//extern crate bitflags;

use std::cell::RefCell;
use std::os::raw::{c_char, c_void};

pub mod qttypes;
pub use qttypes::*;

cpp!{{
    #include <qmetaobject_rust.hpp>
}}

#[doc(hidden)]
pub struct QObjectCppWrapper {
    ptr: *mut c_void,
}
impl Drop for QObjectCppWrapper {
    fn drop(&mut self) {
        let ptr = self.ptr;
        unsafe { cpp!([ptr as "QObject*"] {
            // The event 513 is catched by RustObject and deletes the object.
            QEvent e(QEvent::Type(513));
            if (ptr)
                ptr->event(&e);
        }) };
    }
}
impl Default for QObjectCppWrapper {
    fn default() -> QObjectCppWrapper {
        QObjectCppWrapper {
            ptr: std::ptr::null_mut(),
        }
    }
}
impl QObjectCppWrapper {
    pub fn get(&self) -> *mut c_void {
        self.ptr
    }
    pub fn set(&mut self, val: *mut c_void) {
        self.ptr = val;
    }
}

#[doc(hidden)]
#[repr(C)]
pub struct QObjectDescription {
    pub size: usize,
    pub meta_object: *const QMetaObject,
    pub create: unsafe extern "C" fn(pinned_object: *const c_void, trait_object_ptr: *const c_void)
        -> *mut c_void,
    pub qml_construct: unsafe extern "C" fn(
        mem: *mut c_void,
        pinned_object: *const c_void,
        trait_object_ptr: *const c_void,
        extra_destruct: extern "C" fn(*mut c_void),
    ),
    pub get_rust_refcell: unsafe extern "C" fn(*mut c_void) -> *const RefCell<dyn QObject>,
}

/// Trait that is implemented by the QObject custom derive macro
///
/// Do not implement this trait yourself, use `#[derive(QObject)]`.
///
/// The method of this trait fits into two categories: the ones that are re-implemented by
/// the custom derive, and the ones that are used by this macro and need to be implemented
/// by other QObject-like trait which you use in the qt_base_class! macro.
pub trait QObject {
    // Functions re-implemented by the custom derive:

    /// Returns a pointer to a meta object
    fn meta_object(&self) -> *const QMetaObject;
    /// Returns a pointer to a meta object
    fn static_meta_object() -> *const QMetaObject
    where
        Self: Sized;
    /// return a C++ pointer to the QObject*  (can be null if not yet initialized)
    fn get_cpp_object(&self) -> *mut c_void;
    /// Construct the C++ Object.
    ///
    /// Note, once this function is called, the object must not be moved in memory.
    unsafe fn cpp_construct(pined: &RefCell<Self>) -> *mut c_void
    where
        Self: Sized;
    /// Construct the C++ Object, suitable for callbacks to construct QML objects.
    unsafe fn qml_construct(
        pined: &RefCell<Self>,
        mem: *mut c_void,
        extra_destruct: extern "C" fn(*mut c_void),
    ) where
        Self: Sized;
    /// Return the size of the C++ object
    fn cpp_size() -> usize
    where
        Self: Sized;
    /// Return a rust object belonging to a C++ object
    unsafe fn get_from_cpp<'a>(p: *mut c_void) -> QObjectPinned<'a, Self>
    where
        Self: Sized;

    // Part of the trait structure that sub trait must have.
    // Copy/paste this code replacing QObject with the type.

    /// Returns a QObjectDescription for this type
    fn get_object_description() -> &'static QObjectDescription where Self:Sized {
        unsafe { &*cpp!([]-> *const QObjectDescription as "RustObjectDescription const*" {
            return rustObjectDescription<RustObject<QObject>>();
        } ) }
    }
}
impl dyn QObject {
    /// Creates a C++ object and construct a QVariant containing a pointer to it.
    ///
    /// The cpp_construct function must already have been called.
    ///
    /// FIXME: should probably not be used. Prefer using a QmlEngine::new_qobject.
    /// QVariant is unsafe as it does not manage life time
    pub unsafe fn as_qvariant(&self) -> QVariant {
        let self_ = self.get_cpp_object();
        cpp!{[self_ as "QObject*"] -> QVariant as "QVariant"  {
            return QVariant::fromValue(self_);
        }}
    }


    /// See Qt documentation for QObject::destroyed
    pub fn destroyed_signal() -> CppSignal<fn()> {
        unsafe { CppSignal::new(cpp!([] -> SignalCppRepresentation as "SignalCppRepresentation"  {
            return &QObject::destroyed;
        }))}
    }

    /// See Qt documentation for QObject::setObjectName
    // FIXME. take self by special reference?  panic if cpp_object does not exist?
    pub fn set_object_name(&self, name: QString) {
        let self_ = self.get_cpp_object();
        unsafe {cpp!([self_ as "QObject*", name as "QString"] {
            if (self_) self_->setObjectName(std::move(name));
        })}
    }
    /// See Qt documentation for QObject::objectNameChanged
    pub fn object_name_changed_signal() -> CppSignal<fn(QString)> {
        unsafe {CppSignal::new(cpp!([] -> SignalCppRepresentation as "SignalCppRepresentation"  {
            return &QObject::objectNameChanged;
        }))}
    }
}

cpp_class!(unsafe struct QPointerImpl as "QPointer<QObject>");

/// A Wrapper around a QPointer
// (we only need a *const T to support the !Sized case. (Maybe there is a better way)
pub struct QPointer<T: QObject + ?Sized>(QPointerImpl, *const T);
impl<T: QObject + ?Sized> QPointer<T> {
    /// Returns a pointer to the cpp object (null if it was deleted)
    pub fn cpp_ptr(&self) -> *mut c_void {
        let x = &self.0;
        cpp!(unsafe [x as "QPointer<QObject>*"] -> *mut c_void as "QObject*" {
            return x->data();
        })
    }

    /// Returns a reference to the opbject, or None if it was deleted
    pub fn as_ref(&self) -> Option<&T> {
        let x = self.cpp_ptr();
        if x.is_null() {
            None
        } else {
            unsafe { Some(&*self.1) }
        }
    }

    /// Returns true if the object was default constructed or constructed with an object which
    /// is now deleted
    pub fn is_null(&self) -> bool {
        self.cpp_ptr().is_null()
    }
}
impl<T: QObject> QPointer<T> {
    /// Returns a pinned reference to the opbject, or None if it was deleted
    pub fn as_pinned(&self) -> Option<QObjectPinned<T>> {
        let x = self.cpp_ptr();
        if x.is_null() {
            None
        } else {
            Some(unsafe { T::get_from_cpp(x) })
        }
    }
}

impl<'a, T: QObject + ?Sized> From<&'a T> for QPointer<T> {
    /// Creates a QPointer from a reference to a QObject.
    /// The corresponding C++ object must have already been created.
    fn from(obj: &'a T) -> Self {
        let cpp_obj = obj.get_cpp_object();
        QPointer(cpp!(unsafe [cpp_obj as "QObject *"] -> QPointerImpl  as "QPointer<QObject>" {
            return cpp_obj;
        }), obj as *const T)
    }
}

impl<T: QObject> Default for QPointer<T> {
    fn default() -> Self {
        QPointer(Default::default(), std::ptr::null())
    }
}

impl<T: QObject + ?Sized> Clone for QPointer<T> {
    fn clone(&self) -> Self {
        QPointer(self.0.clone(), self.1)
    }
}

/// Same as std::cell::RefMut,  but does not allow to move from
pub struct QObjectRefMut<'b, T: QObject + ?Sized + 'b> {
    old_value: *mut c_void,
    inner: std::cell::RefMut<'b, T>,
}
impl<'b, T: QObject + ?Sized> std::ops::Deref for QObjectRefMut<'b, T> {
    type Target = std::cell::RefMut<'b, T>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl<'b, T: QObject + ?Sized> std::ops::DerefMut for QObjectRefMut<'b, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl<'b, T: QObject + ?Sized + 'b> Drop for QObjectRefMut<'b, T> {
    #[inline]
    fn drop(&mut self) {
        assert_eq!(
            self.old_value,
            self.get_cpp_object(),
            "Internal pointer changed while borrowed"
        );
    }
}

/// A reference to a RefCell<T>, where T is a QObject, which does not move in memory
#[repr(transparent)]
pub struct QObjectPinned<'pin, T: QObject + ?Sized + 'pin>(&'pin RefCell<T>);
impl<'pin, T: QObject + ?Sized + 'pin> Clone for QObjectPinned<'pin, T> {
    fn clone(&self) -> Self { *self }
}
impl<'pin, T: QObject + ?Sized + 'pin> Copy for QObjectPinned<'pin, T> {}

impl<'pin, T: QObject + ?Sized + 'pin> QObjectPinned<'pin, T> {
    /// Borrow the object
    // FIXME: there are too many case for which we want re-entrency after borrowing
    //pub fn borrow(&self) -> std::cell::Ref<T> { self.0.borrow() }
    #[cfg_attr(feature = "cargo-clippy", allow(clippy::should_implement_trait))]
    pub fn borrow(&self) -> &T {
        unsafe { &*self.0.as_ptr() }
    }
    pub fn borrow_mut(&self) -> QObjectRefMut<T> {
        let x = self.0.borrow_mut();
        QObjectRefMut {
            old_value: x.get_cpp_object(),
            inner: x,
        }
    }
    pub fn as_ptr(&self) -> *mut T {
        self.0.as_ptr()
    }
}
impl<'pin, T: QObject + ?Sized + 'pin> QObjectPinned<'pin, T> {
    /// Internal function used from the code generated by the QObject derive macro
    /// unsafe because one must ensure it does not move in memory
    pub unsafe fn new(inner: &'pin RefCell<T>) -> Self {
        QObjectPinned(inner)
    }
}
impl<'pin, T: QObject + 'pin> QObjectPinned<'pin, T> {
    /// Get the pointer ot the C++ Object, or crate it if it was not yet created
    pub fn get_or_create_cpp_object(self) -> *mut c_void {
        let r = unsafe { &*self.0.as_ptr() }.get_cpp_object();
        if r.is_null() {
            unsafe { QObject::cpp_construct(self.0) }
        } else {
            r
        }
    }
}

/// A wrapper around RefCell<T>, whose content cannot be move in memory
pub struct QObjectBox<T: QObject + ?Sized>(Box<RefCell<T>>);
impl<T: QObject> QObjectBox<T> {
    pub fn new(obj: T) -> Self {
        QObjectBox(Box::new(RefCell::new(obj)))
    }
}
impl<T: QObject + ?Sized> QObjectBox<T> {
    pub fn pinned(&self) -> QObjectPinned<T> {
        unsafe { QObjectPinned::new(&self.0) }
    }
}

/// Create the C++ object and return a C++ pointer to a QObject.
///
/// The ownership is given to CPP, the resulting QObject* ptr need to be used somewhere
/// that takes ownership
///
/// Panics if the C++ object was already created.
pub fn into_leaked_cpp_ptr<T: QObject>(obj: T) -> *mut c_void {
    let b = Box::new(RefCell::new(obj));
    let obj_ptr = unsafe { QObject::cpp_construct(&b) };
    std::boxed::Box::into_raw(b);
    obj_ptr
}

/// Trait that is implemented by the QGadget custom derive macro
///
/// Do not implement this trait yourself, use `#[derive(QGadget)]`.
pub trait QGadget {
    /// Returns a pointer to a meta object
    fn meta_object(&self) -> *const QMetaObject;
    /// Returns a pointer to a meta object
    fn static_meta_object() -> *const QMetaObject
    where
        Self: Sized;
}

/// Trait that is implemented by the QEnum custom derive macro
///
/// Do not implement this trait yourself, use `#[derive(QEnum)]`.
pub trait QEnum {
    /// Returns a pointer to a meta object
    fn static_meta_object() -> *const QMetaObject
    where
        Self: Sized;

    type Repr: Copy;

    fn from_raw(raw: Self::Repr) -> Option<Self>
    where
        Self: Sized;

    fn to_raw(&self) -> Self::Repr;
}

#[doc(hidden)]
#[no_mangle]
pub unsafe extern "C" fn RustObject_metaObject(p: *mut RefCell<dyn QObject>) -> *const QMetaObject {
    (*(*p).as_ptr()).meta_object()
}

#[doc(hidden)]
#[no_mangle]
pub unsafe extern "C" fn RustObject_destruct(p: *mut RefCell<dyn QObject>) {
    // We are destroyed from the C++ code, which means that the object was owned by C++ and we
    // can destroy the rust object as well
    let _b = Box::from_raw(p);
}

/// This function is called from the implementation of the signal.
#[doc(hidden)]
pub unsafe fn invoke_signal(
    object: *mut c_void,
    meta: *const QMetaObject,
    id: u32,
    a: &[*mut c_void],
) {
    let a = a.as_ptr();
    cpp!([object as "QObject*", meta as "const QMetaObject*", id as "int", a as "void**"] {
        if (!object) return;
        QMetaObject::activate(object, meta, id, a);
    })
}

/// Same as a C++ QMetaObject.
#[doc(hidden)]
#[repr(C)]
pub struct QMetaObject {
    pub superdata: *const QMetaObject,
    pub string_data: *const u8,
    pub data: *const u32,
    pub static_metacall:
        Option<extern "C" fn(o: *mut c_void, c: u32, idx: u32, a: *const *mut c_void)>,
    pub r: *const c_void,
    pub e: *const c_void,
}
unsafe impl Sync for QMetaObject {}
unsafe impl Send for QMetaObject {}

/// This macro must be used once as a type in a struct that derives from QObject.
/// It is anotate from which QObject like trait it is supposed to derive.
/// the field which it annotate will be an internal property holding a pointer
/// to the actual C++ object
///
/// The trait needs to be like the QObject trait, see the documentation of the QObject trait.
///
/// ```
/// # #[macro_use] extern crate qmetaobject; use qmetaobject::QObject;
/// #[derive(QObject)]
/// struct Foo {
///    base : qt_base_class!(trait QObject),
/// }
/// ```
///
/// Note: in the future, the plan is to extent so you could derive from other struct by doing
/// `base : qt_base_class(struct Foo)`. But this is not yet implemented
#[macro_export]
macro_rules! qt_base_class {
    ($($t:tt)*) => {
        $crate::QObjectCppWrapper
    };
}

/// This macro can be used as a type of a field and can then turn this field in a Qt property.
/// The first parameter is the type of this property. Then we can have the meta keywords similar
/// to these found in Q_PROPERTY.
///
/// Can be used within a struct that derives from QObject or QGadget
///
/// `NOTIFY` followed by the name of a signal that need to be declared separately.
/// `WRITE` followed by the name of a setter. `READ` follow by the name of a getter. Note that
/// these are not mandatory and if no setter or no getter exist, it will set the field.
/// `CONST` is also supported.
///
/// `ALIAS` followed by an identifier allow to give a different name than the actual field name.
///
/// ```
/// # #[macro_use] extern crate qmetaobject; use qmetaobject::QObject;
/// #[derive(QObject)]
/// struct Foo {
///    base : qt_base_class!(trait QObject),
///    foo : qt_property!(u32; NOTIFY foo_changed WRITE set_foo) ,
///    foo_changed: qt_signal!()
/// }
/// impl Foo {
///    fn set_foo(&mut self, val : u32) { self.foo = val; }
/// }
/// ```
#[macro_export]
macro_rules! qt_property {
    ($t:ty $(; $($rest:tt)*)*) => {
        $t
    };
}

/// This macro can be used to declare a method which will become a meta method.
///
/// Inside you can either declare the method signature, or write the full method.
///
/// Can be used within a struct that derives from QObject or QGadget
///
/// ```
/// # #[macro_use] extern crate qmetaobject; use qmetaobject::QObject;
/// #[derive(QObject)]
/// struct Foo {
///    base : qt_base_class!(trait QObject),
///    defined_method : qt_method!(fn defined_method(&self, foo : u32) -> u32 {
///       println!("contents goes here.");
///       return 42;
///    }),
///    outofline_method : qt_method!(fn(&self, foo : u32)-> u32),
/// }
/// impl Foo {
///    fn outofline_method(&mut self, foo : u32) -> u32 {
///       println!("Or here.");
///       return 69;
///    }
/// }
/// ```
#[macro_export]
macro_rules! qt_method {
    ($($t:tt)*) => { ::std::marker::PhantomData<()> };
}

/// Declares a signal
///
/// Inside you can either declare the method signature, or write the full method.
///
/// To be used within a struct that derives from QObject
///
/// ```
/// # #[macro_use] extern crate qmetaobject; use qmetaobject::QObject;
/// #[derive(QObject)]
/// struct Foo {
///    base : qt_base_class!(trait QObject),
///    my_signal : qt_signal!(xx: u32, yy: String),
/// }
/// fn some_code(foo : &mut Foo) {
///    foo.my_signal(42, "42".into()); // emits the signal
/// }
/// ```
#[macro_export]
macro_rules! qt_signal {
    ($($name:ident : $ty:ty),*) => { $crate::RustSignal<fn($($ty),*)> };
    //() => { $crate::RustSignal0 };
    //($a0:ident : $t0:ty) => { $crate::RustSignal1<$t0> };
    //($a0:ident : $t0:ty, $a1:ident : $t1:ty) => { $crate::RustSignal2<$t0,$t1> };
}

/// Equivalent to the Q_PLUGIN_METADATA macro.
///
/// To be used within a struct that derives from QObject, and it should contain a string which is
/// the IID
///
/// ```
/// # #[macro_use] extern crate qmetaobject; use qmetaobject::qtdeclarative::QQmlExtensionPlugin;
/// #[derive(Default, QObject)]
/// struct MyPlugin {
///     base: qt_base_class!(trait QQmlExtensionPlugin),
///     plugin: qt_plugin!("org.qt-project.Qt.QQmlExtensionInterface/1.0")
/// }
/// # impl QQmlExtensionPlugin for MyPlugin { fn register_types(&mut self, uri : &std::ffi::CStr) { } }
/// ```
#[macro_export]
macro_rules! qt_plugin {
    ($($t:tt)*) => { std::marker::PhantomData<()> };
}

cpp!{{
    struct FnBoxWrapper {
        TraitObject fnbox;
        ~FnBoxWrapper() {
            if (fnbox) {
                rust!(FnBoxWrapper_destructor [fnbox : *mut dyn FnMut() as "TraitObject"] {
                    unsafe { let _ = Box::from_raw(fnbox); }
                });
            }
        }
        FnBoxWrapper &operator=(const FnBoxWrapper&) = delete;
#if false && QT_VERSION >= QT_VERSION_CHECK(5, 10, 0)
        FnBoxWrapper(const FnBoxWrapper&) = delete;
#else
        // Prior to Qt 5.10 we can't have move-only wrapper. Just do the auto_ptr kind of hack.
        FnBoxWrapper(const FnBoxWrapper &o) : fnbox(o.fnbox) {  const_cast<FnBoxWrapper &>(o).fnbox = {}; }
#endif
        FnBoxWrapper(FnBoxWrapper &&o) : fnbox(o.fnbox) {  o.fnbox = {}; }
        FnBoxWrapper &operator=(FnBoxWrapper &&o) { std::swap(o.fnbox, fnbox); return *this; }
        void operator()() {
            rust!(FnBoxWrapper_operator [fnbox : *mut dyn FnMut() as "TraitObject"] {
                unsafe { (*fnbox)(); }
            });
        }
    };

    template<typename T>
    static void invokeMethod(QObject *reciever, T &&func) {
#if QT_VERSION >= QT_VERSION_CHECK(5, 10, 0)
        QMetaObject::invokeMethod(reciever, std::forward<T>(func), Qt::QueuedConnection); // does not allow move-only
#else
        // We can't use QTimer::singleShot because "Timers can only be used with threads started with QThread"
        QObject o;
        QObject::connect(&o, &QObject::destroyed, reciever, std::forward<T>(func), Qt::QueuedConnection);
#endif
    }
}}

/// Call the callback once, after a given duration.
pub fn single_shot<F>(interval: std::time::Duration, func: F)
where
    F: FnMut() + 'static,
{
    let func: Box<dyn FnMut()> = Box::new(func);
    let mut func_raw = Box::into_raw(func);
    let interval_ms : u32 = interval.as_secs() as u32 * 1000 + interval.subsec_nanos() * 1e-6 as u32;
    unsafe{ cpp!([interval_ms as "int", mut func_raw as "FnBoxWrapper"] {
        QTimer::singleShot(interval_ms, std::move(func_raw));
    })};
}

/// Create a callback to invoke a queued callback in the current thread.
///
/// Returns a callback that can be called in any thread. Calling the callback will then call the
/// given closure in the current Qt thread.
///
/// If the current thread does no longer have an event loop when the callback is sent, the
/// callback will not be recieved.
///
/// ```
/// # extern crate qmetaobject; use qmetaobject::queued_callback;
/// // in this example, we do not pass '()' as an argument.
/// let callback = queued_callback(|()| println!("hello from main thread"));
/// std::thread::spawn(move || {callback(());}).join();
/// ```
pub fn queued_callback<T: Send, F: FnMut(T) + 'static>(func: F) -> impl Fn(T) + Send + Sync + Clone {
    let current_thread = cpp!(unsafe [] -> QPointerImpl as "QPointer<QThread>" {
        return QThread::currentThread();
    });

    // In this case, it is safe to send the function to another thread, as we will only call it
    // from this thread.
    // We put it in a RefCell so we can call it mutably.
    struct UnsafeSendFn<T>(RefCell<T>);
    unsafe impl<T> Send for UnsafeSendFn<T> {}
    unsafe impl<T> Sync for UnsafeSendFn<T> {}
    // put func in an arc because we need to keep it alive as long as the internal Box<FnMut> is
    // alive. (And we can't just move it there because the returned closure can be called serveral
    // times.
    let func = std::sync::Arc::new(UnsafeSendFn(RefCell::new(func)));

    move |x| {
        let mut x = Some(x); // Workaround the fact we can't have a Box<FnOnce>
        let func = func.clone();
        let func: Box<dyn FnMut()> = Box::new(move || {
            // the borrow_mut could panic if the function was called recursively. This could happen
            // if the event-loop re-enter.
            let f = &mut (*(func.0).borrow_mut());
            if let Some(x) = x.take() {
                f(x);
            };
        });
        let mut func_raw = Box::into_raw(func);
        unsafe{ cpp!([mut func_raw as "FnBoxWrapper", current_thread as "QPointer<QThread>"] {
            if (!current_thread) return;
            if (!qApp || current_thread != qApp->thread()) {
                QObject *reciever = new QObject();
                reciever->moveToThread(current_thread);
                invokeMethod(reciever, std::move(func_raw));
                reciever->deleteLater();
            } else {
                invokeMethod(qApp, std::move(func_raw));
            }
        })};
    }
}

/* Small helper function for Rust_QAbstractItemModel::roleNames */
fn add_to_hash(hash: *mut c_void, key: i32, value: QByteArray) {
    unsafe {
        cpp!([hash as "QHash<int, QByteArray>*", key as "int", value as "QByteArray"]{
        (*hash)[key] = std::move(value);
    })
    }
}

/// Refer to the documentation of Qt::UserRole
pub const USER_ROLE: i32 = 0x0100;

cpp_class!(
/// Wrapper for Qt's QMessageLogContext
pub unsafe struct QMessageLogContext as "QMessageLogContext");
impl QMessageLogContext {
    // Return QMessageLogContext::line
    pub fn line(&self) -> i32 {
        cpp!(unsafe [self as "QMessageLogContext*"] -> i32 as "int" { return self->line; })
    }
    // Return QMessageLogContext::file
    pub fn file(&self) -> &str {
        unsafe {
            let x = cpp!([self as "QMessageLogContext*"] -> *const c_char as "const char*" {
                return self->file;
            });
            if x.is_null() {
                return "";
            }
            std::ffi::CStr::from_ptr(x).to_str().unwrap()
        }
    }
    // Return QMessageLogContext::function
    pub fn function(&self) -> &str {
        unsafe {
            let x = cpp!([self as "QMessageLogContext*"] -> *const c_char as "const char*" {
                return self->function;
            });
            if x.is_null() {
                return "";
            }
            std::ffi::CStr::from_ptr(x).to_str().unwrap()
        }
    }
    // Return QMessageLogContext::category
    pub fn category(&self) -> &str {
        unsafe {
            let x = cpp!([self as "QMessageLogContext*"] -> *const c_char as "const char*" {
                return self->category;
            });
            if x.is_null() {
                return "";
            }
            std::ffi::CStr::from_ptr(x).to_str().unwrap()
        }
    }
}

/// Wrap Qt's QtMsgType enum
#[repr(C)]
#[derive(Debug)]
pub enum QtMsgType {
    QtDebugMsg,
    QtWarningMsg,
    QtCriticalMsg,
    QtFatalMsg,
    QtInfoMsg,
}

/// Wrap qt's qInstallMessageHandler.
/// Useful in order to forward the log to a rust logging framework
pub fn install_message_handler(logger: extern "C" fn(QtMsgType, &QMessageLogContext, &QString)) {
    cpp!(unsafe [logger as "QtMessageHandler"] { qInstallMessageHandler(logger); })
}

pub mod itemmodel;
pub use itemmodel::*;
pub mod listmodel;
pub use listmodel::*;
pub mod qtdeclarative;
pub use qtdeclarative::*;
pub mod qmetatype;
pub use qmetatype::*;
#[macro_use]
pub mod qrc;
pub mod connections;
pub use connections::RustSignal;
pub use connections::{CppSignal, SignalCppRepresentation, connect};
pub mod scenegraph;
pub mod qtquickcontrols2;
pub use qtquickcontrols2::*;
