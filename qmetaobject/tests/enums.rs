extern crate qmetaobject;
use qmetaobject::*;

use std::ffi::CStr;

mod common;
use common::*;

#[derive(QEnum, Clone, Copy, Debug)]
#[repr(i8)]
enum PriceLevel {
    Cheap = -23,
    Standard = 1,
    Expensive = 93,
}

impl Default for PriceLevel {
    fn default() -> Self {
        PriceLevel::Standard
    }
}

mod products {
    use qmetaobject::*;

    #[derive(QEnum, Clone, Copy, Debug)]
    #[repr(u16)]
    pub enum Fruits {
        Banana = 5,
        Apple = 163,
        Peach = std::u16::MAX - 1,
    }

    impl Default for Fruits {
        fn default() -> Self {
            Fruits::Banana
        }
    }
}

#[derive(QObject, Default)]
struct Store {
    base: qt_base_class!(trait QObject),

    level: qt_property!(PriceLevel; NOTIFY level_changed),
    level_changed: qt_signal!(),

    fruit: qt_property!(products::Fruits; NOTIFY fruit_changed),
    fruit_changed: qt_signal!(),

    price_level_of: qt_method!(fn(&self, fruit: products::Fruits) -> PriceLevel),
    assert_apple: qt_method!(fn(&self, fruit: products::Fruits)),
    assert_cheap: qt_method!(fn(&self, level: PriceLevel)),
}

impl Store {
    fn price_level_of(&self, fruit: products::Fruits) -> PriceLevel {
        use products::Fruits;
        let price = match fruit {
            Fruits::Banana => PriceLevel::Cheap,
            Fruits::Apple => PriceLevel::Standard,
            Fruits::Peach => PriceLevel::Expensive,
        };
        println!("{:?} => {:?}", fruit, price);
        price
    }

    fn assert_apple(&self, fruit: products::Fruits) {
        use products::Fruits;
        match fruit {
            Fruits::Apple => {},
            _ => panic!("Expected an apple")
        }
    }

    fn assert_cheap(&self, level: PriceLevel) {
        match level {
            PriceLevel::Cheap => {}
            _ => panic!("Expected cheap")
        }
    }
}

fn register_types() {
    let lib = CStr::from_bytes_with_nul(b"Business\0").unwrap();
    qml_register_enum::<PriceLevel>(
        lib,
        1,
        0,
        CStr::from_bytes_with_nul(b"PriceLevel\0").unwrap(),
    );
    qml_register_enum::<products::Fruits>(
        lib,
        1,
        0,
        CStr::from_bytes_with_nul(b"Fruits\0").unwrap(),
    );
    qml_register_type::<Store>(
        lib,
        1,
        0,
        CStr::from_bytes_with_nul(b"Store\0").unwrap(),
    );
}

#[test]
fn read_write_properties() {
    register_types();
    let my_obj = Store::default();
    assert!(do_test(
        my_obj,
        "import Business 1.0
        Item {
            function doTest() {
                console.log(_obj.level, PriceLevel.Standard)
                if(_obj.level != PriceLevel.Standard) {
                    return false;
                }
                _obj.level = PriceLevel.Cheap;
                console.log(_obj.level, PriceLevel.Cheap)
                if(_obj.level != PriceLevel.Cheap) {
                    return false;
                }

                console.log(_obj.fruit, Fruits.Banana)
                if(_obj.fruit != Fruits.Banana) {
                    return false;
                }
                _obj.fruit = Fruits.Apple;
                console.log(_obj.fruit, Fruits.Apple)
                if(_obj.fruit != Fruits.Apple) {
                    return false;
                }
                _obj.fruit = Fruits.Peach;
                console.log(_obj.fruit, Fruits.Peach)
                if(_obj.fruit != Fruits.Peach) {
                    return false;
                }
                return true;
            }
        }
        "
    ));
}


#[test]
fn calls() {
    register_types();
    let my_obj = Store::default();
    assert!(do_test(
        my_obj,
        "import Business 1.0
        Item {
            function doTest() {
                _obj.assert_apple(Fruits.Apple);
                _obj.assert_cheap(PriceLevel.Cheap);

                if(_obj.price_level_of(Fruits.Banana) != PriceLevel.Cheap) {
                    return false;
                }
                console.log('apple');
                if(_obj.price_level_of(Fruits.Apple) != PriceLevel.Standard) {
                    return false;
                }
                console.log('peach');
                if(_obj.price_level_of(Fruits.Peach) != PriceLevel.Expensive) {
                    return false;
                }
                return true;
            }
        }
        "
    ));
}