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
use super::*;

fn register_metatype_common<T: QMetaType>(
    name: *const std::os::raw::c_char,
    metaobject: *const QMetaObject,
    is_enum: bool,
) -> i32 {
    use std::any::TypeId;
    use std::collections::{HashMap, HashSet};
    use std::ffi::{CStr, CString};
    use std::sync::Mutex;

    lazy_static! {
        static ref HASHMAP: Mutex<HashMap<TypeId, (i32, HashSet<CString>)>> =
            Mutex::new(HashMap::new());
    };

    let mut h = HASHMAP.lock().unwrap_or_else(|e| e.into_inner());
    let e = h.entry(TypeId::of::<T>()).or_insert_with(|| {
        let size = std::mem::size_of::<T>() as u32;

        extern "C" fn deleter_fn<T>(v: *mut T) { unsafe { Box::from_raw(v); } };
        let deleter_fn: extern "C" fn(v: *mut T) = deleter_fn;

        extern "C" fn creator_fn<T: Default + Clone>(c: *const T) -> *const T {
            if c.is_null() {
                Box::into_raw(Box::new(Default::default()))
            } else {
                Box::into_raw(Box::new(unsafe { (*c).clone() }))
            }
        };
        let creator_fn: extern "C" fn(c: *const T) -> *const T = creator_fn;

        extern "C" fn destructor_fn<T>(ptr: *mut T) {
            unsafe {
                std::ptr::read(ptr);
            }
        };
        let destructor_fn: extern "C" fn(ptr: *mut T) = destructor_fn;

        extern "C" fn constructor_fn<T: Default + Clone>(dst: *mut T, c: *const T) -> *mut T {
            unsafe {
                let n = if c.is_null() {
                    Default::default()
                } else {
                    (*c).clone()
                };
                std::ptr::write(dst, n);
            }
            dst
        };
        let constructor_fn: extern "C" fn(ptr: *mut T, c: *const T) -> *mut T = constructor_fn;

        let name = CString::new(format!("{:?}", TypeId::of::<T>())).unwrap();
        let name = name.as_ptr();
        let type_id = cpp!(unsafe [name as "const char*", size as "int", deleter_fn as "QMetaType::Deleter",
                creator_fn as "QMetaType::Creator", destructor_fn as "QMetaType::Destructor",
                constructor_fn as "QMetaType::Constructor", metaobject as "const QMetaObject*",
                is_enum as "bool"] -> i32 as "int" {
            auto type_flag = is_enum ? QMetaType::IsEnumeration : QMetaType::IsGadget;
            QMetaType::TypeFlags extraFlag(metaobject ? type_flag : 0);
            return QMetaType::registerType(metaobject ? metaobject->className() : name, deleter_fn, creator_fn, destructor_fn,
                constructor_fn, size,
                QMetaType::NeedsConstruction | QMetaType::NeedsDestruction | QMetaType::MovableType | extraFlag,
                metaobject);
        });

        if T::CONVERSION_TO_STRING.is_some() {
            extern "C" fn converter_fn<T : QMetaType>(_ : *const c_void, src: &T, dst : *mut QString) -> bool {
                unsafe { std::ptr::write(dst, (T::CONVERSION_TO_STRING.unwrap())(src)) };
                true
            }
            let converter_fn: extern "C" fn(*const c_void, &T, *mut QString) -> bool = converter_fn;
            cpp!( unsafe [type_id as "int", converter_fn as "QtPrivate::AbstractConverterFunction::Converter"] {
                //NOTE: the ConverterFunctor are gonna be leaking (in Qt, they are suppoed to be allocated in static storage
                auto c = new QtPrivate::ConverterFunctor<TraitObject, TraitObject, TraitObject>(converter_fn);
                if (!c->registerConverter(type_id, QMetaType::QString))
                    delete c;
            });
        };

        if T::CONVERSION_FROM_STRING.is_some() {
            extern "C" fn converter_fn<T : QMetaType>(_ : *const c_void, src : &QString, dst : *mut T) -> bool {
                unsafe { std::ptr::write(dst, (T::CONVERSION_FROM_STRING.unwrap())(src)) };
                true
            }
            let converter_fn: extern "C" fn(*const c_void, &QString, *mut T) -> bool = converter_fn;
            cpp!(unsafe [type_id as "int", converter_fn as "QtPrivate::AbstractConverterFunction::Converter"] {
                auto c = new QtPrivate::ConverterFunctor<TraitObject, TraitObject, TraitObject>(converter_fn);
                if (!c->registerConverter(QMetaType::QString, type_id))
                    delete c;
            });
        };
        (type_id, HashSet::new())
    });
    let id = e.0;
    if !name.is_null() && !e.1.contains(unsafe { CStr::from_ptr(name) }) {
        let x = cpp!(unsafe [name as "const char*", id as "int"] -> i32 as "int" {
            if (int exist = QMetaType::type(name)) {
                if (exist != id) {
                    qWarning("Attempt to register %s as a typedef of %s, while it was already registered as %s",
                        name, QMetaType::typeName(id), QMetaType::typeName(exist));
                }
                return exist;
            }
            return QMetaType::registerTypedef(name, id);
        });
        assert_eq!(
            x, id,
            "Attempt to register the same type with different name"
        );
        e.1.insert(unsafe { CStr::from_ptr(name) }.to_owned());
    }
    id
}

fn register_metatype_qobject<T: QObject>() -> i32 {
    let metaobject = T::static_meta_object();
    unsafe {
        cpp!([metaobject as "const QMetaObject*"] -> i32 as "int" {
            return QMetaType::registerType(metaobject->className(),
                [](void*p) { delete static_cast<void**>(p); },
                [](const void*p) -> void* { using T = void*; return new T{ p ? *static_cast<const T*>(p) : nullptr}; },
                QtMetaTypePrivate::QMetaTypeFunctionHelper<void*>::Destruct,
                QtMetaTypePrivate::QMetaTypeFunctionHelper<void*>::Construct,
                sizeof(void*),
                QMetaType::MovableType | QMetaType::PointerToQObject,
                metaobject);
        })
    }
}

pub fn register_metatype_qenum<T: QEnum + QMetaType>(
    name: *const std::os::raw::c_char,
) -> i32 {
    register_metatype_common::<T>(name, T::static_meta_object(), true)
}

pub fn enum_to_qvariant<T: QEnum + QMetaType>(e: &T) -> QVariant {
    let id: i32 = T::id();
    let raw = e.to_raw();
    let raw_ptr = &raw;
    cpp!(unsafe [id as "int", raw_ptr as "const void*"] -> QVariant as "QVariant" {
        return QVariant(id, raw_ptr);
    })
}

fn qvariant_internal_ptr(var_ptr: *mut QVariant, id: i32) -> *const c_void {
    cpp!(unsafe [var_ptr as "QVariant*", id as "int"] -> *const c_void as "const void*" {
        return var_ptr->canConvert(id) && var_ptr->convert(id) ? var_ptr->constData() : nullptr;
    })
}

pub fn enum_from_qvariant<T: QEnum + QMetaType>(mut variant: QVariant) -> Option<T> {
    let id: i32 = T::id();
    let var_ptr = &mut variant as *mut QVariant;
    let ptr = qvariant_internal_ptr(var_ptr, id);
    if ptr.is_null() {
        None
    } else {
        let raw = unsafe{ *(ptr as *const T::Repr) };
        T::from_raw(raw)
    }
}

fn register_metatype_qmetatype<T: QMetaType>(
    name: *const std::os::raw::c_char,
    gadget_metaobject: *const QMetaObject,
) -> i32 {
    register_metatype_common::<T>(name, gadget_metaobject, false)
}

/// Implement this trait for type that should be known to the QMetaObject system
///
/// Once implemented for a type, it can be used as a type of a qt_property! or
/// as a parameter of a qt_method!
///
/// ```
/// # use ::qmetaobject::QMetaType;
/// #[derive(Default, Clone)]
/// struct MyStruct(u32, String);
/// impl QMetaType for MyStruct {}
/// ```
pub trait QMetaType: Clone + Default + 'static {
    /// Registers the type.
    ///
    /// See the Qt documentation of qRegisterMetaType()
    ///
    /// The default implementation should work for most types
    fn register(name: Option<&std::ffi::CStr>) -> i32 {
        register_metatype_qmetatype::<Self>(
            name.map_or(std::ptr::null(), |x| x.as_ptr()),
            std::ptr::null(),
        )
    }

    fn id() -> i32 {
        Self::register(None)
    }

    /// Returns a QVariant containing a copy of this object
    fn to_qvariant(&self) -> QVariant {
        let id: i32 = Self::id();
        cpp!(unsafe [self as "const void*", id as "int"] -> QVariant as "QVariant" {
            return QVariant(id, self);
        })
    }

    /// Attempt to convert from a QVariant to this type.
    fn from_qvariant(mut variant: QVariant) -> Option<Self> {
        let id: i32 = Self::id();
        let var_ptr = &mut variant as *mut QVariant;
        let ptr = qvariant_internal_ptr(var_ptr, id);
        if ptr.is_null() {
            None
        } else {
            Some(unsafe {
                (*(ptr as *const Self)).clone()
            })
        }
    }

    /// If this is set to a function, it enable the conversion to and from QString
    const CONVERSION_TO_STRING: Option<fn(&Self) -> QString> = None;
    const CONVERSION_FROM_STRING: Option<fn(&QString) -> Self> = None;
}

/// QGadget are automatically QMetaType
impl<T: QGadget> QMetaType for T
where
    T: Clone + Default + 'static,
{
    fn register(name: Option<&std::ffi::CStr>) -> i32 {
        register_metatype_qmetatype::<T>(
            name.map_or(std::ptr::null(), |x| x.as_ptr()),
            T::static_meta_object(),
        )
    }
}

impl QMetaType for String {
    const CONVERSION_TO_STRING: Option<fn(&Self) -> QString> = Some(|s| QString::from(&*s as &str));
    const CONVERSION_FROM_STRING: Option<fn(&QString) -> Self> = Some(|s| s.to_string());
}

macro_rules! qdeclare_builtin_metatype {
    ($name:ty => $value:expr) => {
        impl QMetaType for $name {
            fn register(_name: Option<&std::ffi::CStr>) -> i32 {
                $value
            }
        }
    };
}
qdeclare_builtin_metatype!{()   => 43}
qdeclare_builtin_metatype!{bool => 1}
qdeclare_builtin_metatype!{i32  => 2}
qdeclare_builtin_metatype!{u32  => 3}
qdeclare_builtin_metatype!{i64  => 4}
qdeclare_builtin_metatype!{u64  => 5}
qdeclare_builtin_metatype!{f64  => 6}
qdeclare_builtin_metatype!{i16  => 33}
qdeclare_builtin_metatype!{i8   => 34}
qdeclare_builtin_metatype!{u16  => 36}
qdeclare_builtin_metatype!{u8   => 37}
qdeclare_builtin_metatype!{f32  => 38}
//qdeclare_builtin_metatype!{"*c_void" => 31,
qdeclare_builtin_metatype!{QVariantList  => 9}
qdeclare_builtin_metatype!{QString => 10}
qdeclare_builtin_metatype!{QByteArray => 12}
qdeclare_builtin_metatype!{QRectF => 20}
qdeclare_builtin_metatype!{QPointF => 26}
//qdeclare_builtin_metatype!{QVariant => 41}
impl QMetaType for QVariant {
    fn register(_name: Option<&std::ffi::CStr>) -> i32 {
        41
    }
    fn to_qvariant(&self) -> QVariant {
        self.clone()
    }
    fn from_qvariant(variant: QVariant) -> Option<Self> {
        Some(variant)
    }
}
qdeclare_builtin_metatype!{QModelIndex => 42}
qdeclare_builtin_metatype!{QColor => 67}
qdeclare_builtin_metatype!{QImage => 70}

#[cfg(target_pointer_width = "32")]
qdeclare_builtin_metatype!{isize  => 2} // That's QMetaType::Int
#[cfg(target_pointer_width = "32")]
qdeclare_builtin_metatype!{usize  => 3} // That's QMetaType::UInt
#[cfg(target_pointer_width = "64")]
qdeclare_builtin_metatype!{isize  => 4} // That's QMetaType::LongLong
#[cfg(target_pointer_width = "64")]
qdeclare_builtin_metatype!{usize  => 5} // That's QMetaType::ULongLong

/// Internal trait used to pass or read the type in a Q_PROPERTY
///
/// Don't implement this trait, implement the QMetaType trait.
pub trait PropertyType {
    fn register_type(name: &std::ffi::CStr) -> i32;
    // Note: this is &mut self becauser of the lazy initialization of the QObject* for the QObject impl
    unsafe fn pass_to_qt(&mut self, a: *mut c_void);
    unsafe fn read_from_qt(a: *const c_void) -> Self;
}

impl<T: QMetaType> PropertyType for T
where
    T: QMetaType,
{
    unsafe fn pass_to_qt(&mut self, a: *mut c_void) {
        let r = a as *mut Self;
        if !r.is_null() {
            *r = self.clone();
        }
    }

    unsafe fn read_from_qt(a: *const c_void) -> Self {
        let r = a as *const Self;
        (*r).clone()
    }

    fn register_type(name: &std::ffi::CStr) -> i32 {
        <T as QMetaType>::register(Some(name))
    }
}

impl<T> PropertyType for ::std::cell::RefCell<T>
where
    T: QObject,
{
    fn register_type(_name: &::std::ffi::CStr) -> i32 {
        register_metatype_qobject::<T>()
    }
    unsafe fn pass_to_qt(&mut self, a: *mut ::std::os::raw::c_void) {
        let pinned = QObjectPinned::new(self);
        let r = a as *mut *const ::std::os::raw::c_void;
        *r = pinned.get_or_create_cpp_object()
    }

    unsafe fn read_from_qt(_a: *const ::std::os::raw::c_void) -> Self {
        panic!("Cannot write into an Object property");
    }
}

impl<T> PropertyType for QPointer<T>
where
    T: QObject,
{
    fn register_type(_name: &::std::ffi::CStr) -> i32 {
        register_metatype_qobject::<T>()
    }
    unsafe fn pass_to_qt(&mut self, a: *mut ::std::os::raw::c_void) {
        let pinned = self.as_pinned();
        let r = a as *mut *const ::std::os::raw::c_void;
        match pinned {
            Some(pinned) => *r = pinned.get_or_create_cpp_object(),
            None => *r = std::ptr::null(),
        }
    }

    unsafe fn read_from_qt(a: *const ::std::os::raw::c_void) -> Self {
        let r = a as *const *mut ::std::os::raw::c_void;
        if a.is_null() || (*r).is_null() {
            Self::default()
        } else {
            let obj = T::get_from_cpp(*r);
            obj.borrow().into()
        }
    }
}

#[test]
fn test_qmetatype() {
    #[derive(Default, Clone, Debug, Eq, PartialEq)]
    struct MyInt {
        x: u32,
    };
    impl QMetaType for MyInt {};

    assert_eq!(
        MyInt::register(Some(&std::ffi::CString::new("MyInt").unwrap())),
        MyInt::id()
    );
    let m42 = MyInt { x: 42 };
    let m43 = MyInt { x: 43 };

    assert_eq!(
        Some(m42.clone()),
        MyInt::from_qvariant(m42.clone().to_qvariant())
    );
    assert_eq!(
        Some(m43.clone()),
        MyInt::from_qvariant(m43.clone().to_qvariant())
    );

    assert_eq!(None, u32::from_qvariant(m43.to_qvariant()));
    assert_eq!(None, MyInt::from_qvariant(45u32.to_qvariant()));
    assert_eq!(Some(45), u32::from_qvariant(45u32.to_qvariant()));
}

#[test]
#[should_panic(expected = "Attempt to register the same type with different name")]
fn test_qmetatype_register_wrong_type1() {
    #[derive(Default, Clone, Debug, Eq, PartialEq)]
    struct MyType {};
    impl QMetaType for MyType {};
    // registering with the name of an existing type should panic
    MyType::register(Some(&std::ffi::CString::new("QString").unwrap()));
}

#[test]
#[should_panic(expected = "Attempt to register the same type with different name")]
fn test_qmetatype_register_wrong_type2() {
    #[derive(Default, Clone, Debug, Eq, PartialEq)]
    struct MyType {};
    impl QMetaType for MyType {};
    String::register(Some(&std::ffi::CString::new("String").unwrap()));
    // registering with the name of an existing type should panic
    MyType::register(Some(&std::ffi::CString::new("String").unwrap()));
}
