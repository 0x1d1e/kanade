/*
 * fixed conversion units need no network; temperatures convert through Celsius and customary
 * volumes are US measures. Match whole names case-insensitively (`m` never matches `cm`)
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dimension {
    // meters
    Length,

    // grams
    Mass,

    // liters
    Volume,

    // square meters
    Area,

    // meters per second
    Speed,

    // seconds
    Time,

    // bytes
    Data,

    // degrees Celsius, Fahrenheit and kelvin
    Temperature,
}

#[derive(Debug)]
pub struct Unit {
    // what it is called; the first is how a result writes it
    names: &'static [&'static str],
    dimension: Dimension,

    // how many of the dimension's base unit make one of it; unused by a temperature
    per: f64,
}

impl Unit {
    pub fn label(&self) -> &'static str {
        self.names[0]
    }
}

const fn unit(names: &'static [&'static str], dimension: Dimension, per: f64) -> Unit {
    Unit {
        names,
        dimension,
        per,
    }
}

use Dimension::{Area, Data, Length, Mass, Speed, Temperature, Time, Volume};

// one unit to a line, as a table reads
#[rustfmt::skip]
const UNITS: &[Unit] = &[
    unit(&["mm", "millimeter", "millimeters", "millimetre", "millimetres"], Length, 0.001),
    unit(&["cm", "centimeter", "centimeters", "centimetre", "centimetres"], Length, 0.01),
    unit(&["m", "meter", "meters", "metre", "metres"], Length, 1.0),
    unit(&["km", "kilometer", "kilometers", "kilometre", "kilometres"], Length, 1000.0),
    unit(&["in", "inch", "inches"], Length, 0.0254),
    unit(&["ft", "foot", "feet"], Length, 0.3048),
    unit(&["yd", "yard", "yards"], Length, 0.9144),
    unit(&["mi", "mile", "miles"], Length, 1609.344),
    unit(&["nmi", "nautical mile", "nautical miles"], Length, 1852.0),
    unit(&["mg", "milligram", "milligrams"], Mass, 0.001),
    unit(&["g", "gram", "grams"], Mass, 1.0),
    unit(&["kg", "kilogram", "kilograms"], Mass, 1000.0),
    unit(&["t", "tonne", "tonnes"], Mass, 1_000_000.0),
    unit(&["oz", "ounce", "ounces"], Mass, 28.349_523_125),
    unit(&["lb", "lbs", "pound", "pounds"], Mass, 453.592_37),
    unit(&["st", "stone"], Mass, 6_350.293_18),
    unit(&["mL", "milliliter", "milliliters", "millilitre", "millilitres"], Volume, 0.001),
    unit(&["cL", "centiliter", "centiliters", "centilitre", "centilitres"], Volume, 0.01),
    unit(&["L", "liter", "liters", "litre", "litres"], Volume, 1.0),
    unit(&["tsp", "teaspoon", "teaspoons"], Volume, 0.004_928_921_593_75),
    unit(&["tbsp", "tablespoon", "tablespoons"], Volume, 0.014_786_764_781_25),
    unit(&["fl oz", "floz", "fluid ounce", "fluid ounces"], Volume, 0.029_573_529_562_5),
    unit(&["cup", "cups"], Volume, 0.236_588_236_5),
    unit(&["pt", "pint", "pints"], Volume, 0.473_176_473),
    unit(&["qt", "quart", "quarts"], Volume, 0.946_352_946),
    unit(&["gal", "gallon", "gallons"], Volume, 3.785_411_784),
    unit(&["cm²", "cm2", "sqcm"], Area, 0.0001),
    unit(&["m²", "m2", "sqm"], Area, 1.0),
    unit(&["ha", "hectare", "hectares"], Area, 10_000.0),
    unit(&["km²", "km2", "sqkm"], Area, 1_000_000.0),
    unit(&["ft²", "ft2", "sqft"], Area, 0.092_903_04),
    unit(&["acre", "acres"], Area, 4_046.856_422_4),
    unit(&["mi²", "mi2", "sqmi"], Area, 2_589_988.110_336),
    unit(&["m/s", "mps"], Speed, 1.0),
    unit(&["km/h", "kph", "kmh", "kmph"], Speed, 1000.0 / 3600.0),
    unit(&["mph"], Speed, 0.447_04),
    unit(&["kn", "knot", "knots"], Speed, 1852.0 / 3600.0),
    unit(&["ft/s", "fps"], Speed, 0.3048),
    unit(&["ms", "millisecond", "milliseconds"], Time, 0.001),
    unit(&["s", "sec", "secs", "second", "seconds"], Time, 1.0),
    unit(&["min", "mins", "minute", "minutes"], Time, 60.0),
    unit(&["h", "hr", "hrs", "hour", "hours"], Time, 3600.0),
    unit(&["d", "day", "days"], Time, 86_400.0),
    unit(&["wk", "week", "weeks"], Time, 604_800.0),
    unit(&["bit", "bits"], Data, 0.125),
    unit(&["B", "byte", "bytes"], Data, 1.0),
    unit(&["KB", "kilobyte", "kilobytes"], Data, 1e3),
    unit(&["MB", "megabyte", "megabytes"], Data, 1e6),
    unit(&["GB", "gigabyte", "gigabytes"], Data, 1e9),
    unit(&["TB", "terabyte", "terabytes"], Data, 1e12),
    unit(&["KiB", "kibibyte", "kibibytes"], Data, 1024.0),
    unit(&["MiB", "mebibyte", "mebibytes"], Data, 1_048_576.0),
    unit(&["GiB", "gibibyte", "gibibytes"], Data, 1_073_741_824.0),
    unit(&["TiB", "tebibyte", "tebibytes"], Data, 1_099_511_627_776.0),
    unit(&["°C", "C", "celsius"], Temperature, 1.0),
    unit(&["°F", "F", "fahrenheit"], Temperature, 1.0),
    unit(&["K", "kelvin"], Temperature, 1.0),
];

// the unit named `name`, whole
pub fn named(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();

    UNITS
        .iter()
        .find(|unit| unit.names.iter().any(|known| known.to_lowercase() == name))
}

/*
 * the unit that `text` ends with, and what comes before it: the longest name that starts at a
 * word, so `10 fl oz` ends in `fl oz` and not `oz`, and `10 cm` in `cm`, not `m`
 */
pub fn ending(text: &str) -> Option<(&str, &'static Unit)> {
    let lower = text.to_lowercase();

    // lowercasing may change a length, so only text it leaves alone is cut by the name's
    if lower.len() != text.len() {
        return None;
    }

    UNITS
        .iter()
        .flat_map(|unit| {
            unit.names
                .iter()
                .map(move |name| (name.to_lowercase(), unit))
        })
        .filter(|(name, _)| lower.ends_with(name.as_str()))
        .filter_map(|(name, unit)| {
            let before = text.get(..text.len() - name.len())?;

            let at_word = before
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphabetic());

            at_word.then_some((name.len(), before, unit))
        })
        .max_by_key(|(length, ..)| *length)
        .map(|(_, before, unit)| (before, unit))
}

// `value` of `from` as `to`; none for units of different dimensions
pub fn convert(value: f64, from: &Unit, to: &Unit) -> Option<f64> {
    if from.dimension != to.dimension {
        return None;
    }

    if from.dimension == Temperature {
        return Some(from_celsius(to_celsius(value, from), to));
    }

    Some(value * from.per / to.per)
}

fn to_celsius(value: f64, unit: &Unit) -> f64 {
    match unit.label() {
        "°F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        _ => value,
    }
}

fn from_celsius(celsius: f64, unit: &Unit) -> f64 {
    match unit.label() {
        "°F" => celsius * 9.0 / 5.0 + 32.0,
        "K" => celsius + 273.15,
        _ => celsius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_unit_and_a_unit_has_a_size() {
        let mut seen = std::collections::BTreeSet::new();

        for unit in UNITS {
            assert!(unit.per.is_finite() && unit.per > 0.0, "{}", unit.label());

            for name in unit.names {
                assert!(seen.insert(name.to_lowercase()), "{name} names two units");
            }
        }
    }

    fn converted(value: f64, from: &str, to: &str) -> Option<f64> {
        convert(value, named(from)?, named(to)?)
    }

    // to the digits the calculator shows
    fn near(value: Option<f64>, want: f64) {
        let value = value.expect("a conversion");

        assert!(
            (value - want).abs() <= want.abs().max(1.0) * 1e-12,
            "{value} is not {want}"
        );
    }

    #[test]
    fn lengths_masses_and_volumes_convert_by_their_definitions() {
        near(converted(10.0, "cm", "in"), 3.937_007_874_015_748);
        near(converted(1.0, "mi", "km"), 1.609_344);
        near(converted(1.0, "ft", "in"), 12.0);
        near(converted(1.0, "yd", "ft"), 3.0);
        near(converted(1.0, "nmi", "m"), 1852.0);
        near(converted(1.0, "kg", "lb"), 2.204_622_621_848_776);
        near(converted(16.0, "oz", "lb"), 1.0);
        near(converted(1.0, "st", "lb"), 14.0);
        near(converted(1.0, "gal", "qt"), 4.0);
        near(converted(1.0, "qt", "pt"), 2.0);
        near(converted(1.0, "cup", "fl oz"), 8.0);
        near(converted(3.0, "tsp", "tbsp"), 1.0);
        near(converted(1.0, "l", "ml"), 1000.0);
    }

    #[test]
    fn speeds_times_data_and_areas_convert() {
        near(converted(60.0, "mph", "km/h"), 96.560_64);
        near(converted(1.0, "kn", "km/h"), 1.852);
        near(converted(90.0, "min", "h"), 1.5);
        near(converted(1.0, "wk", "d"), 7.0);
        near(converted(1.0, "gib", "mib"), 1024.0);
        near(converted(1.0, "gb", "mb"), 1000.0);
        near(converted(8.0, "bits", "b"), 1.0);
        near(converted(1.0, "ha", "m²"), 10_000.0);
        near(converted(1.0, "acre", "ft2"), 43_560.0);
        near(converted(1.0, "mi2", "acres"), 640.0);
    }

    #[test]
    fn temperatures_convert_through_celsius_and_meet_at_their_fixed_points() {
        near(converted(100.0, "c", "f"), 212.0);
        near(converted(32.0, "f", "c"), 0.0);
        near(converted(-40.0, "°f", "°c"), -40.0);
        near(converted(0.0, "celsius", "k"), 273.15);
        near(converted(0.0, "k", "f"), -459.67);
    }

    #[test]
    fn units_of_other_dimensions_do_not_convert() {
        assert_eq!(converted(1.0, "kg", "m"), None);
        assert_eq!(converted(1.0, "c", "m"), None);
        assert_eq!(converted(1.0, "foo", "m"), None);
    }

    #[test]
    fn a_unit_is_found_at_the_end_of_a_word_only() {
        let at = |text| ending(text).map(|(before, unit)| (before.to_owned(), unit.label()));

        assert_eq!(at("10 cm"), Some((String::from("10 "), "cm")));
        assert_eq!(at("10cm"), Some((String::from("10"), "cm")));
        assert_eq!(at("2 M"), Some((String::from("2 "), "m")));
        assert_eq!(at("3 fl oz"), Some((String::from("3 "), "fl oz")));
        assert_eq!(at("5 xm"), None);
        assert_eq!(at("12"), None);
    }
}
