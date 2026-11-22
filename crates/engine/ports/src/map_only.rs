//! Settings objects read from a JSON object only.

/// `Serialize` and `Deserialize` for a type that derives them with
/// `#[serde(remote = "Self")]`, deserializing from a map only. Serde's
/// derive also reads a struct from a sequence, by position: for a type whose
/// fields all have defaults, `[]` would be read as the defaults (a request
/// or file holding `"processing": []` would reset processing). The map goes
/// through to the derived code as it is, so `deny_unknown_fields`, defaults
/// and the path of an error are kept.
#[macro_export]
macro_rules! map_only {
    ($type:ty, $expecting:literal) => {
        impl $crate::__serde::Serialize for $type {
            fn serialize<S: $crate::__serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::std::result::Result<S::Ok, S::Error> {
                <$type>::serialize(self, serializer)
            }
        }

        impl<'de> $crate::__serde::Deserialize<'de> for $type {
            fn deserialize<D: $crate::__serde::Deserializer<'de>>(
                deserializer: D,
            ) -> ::std::result::Result<Self, D::Error> {
                struct Map;
                impl<'de> $crate::__serde::de::Visitor<'de> for Map {
                    type Value = $type;
                    fn expecting(
                        &self,
                        formatter: &mut ::std::fmt::Formatter<'_>,
                    ) -> ::std::fmt::Result {
                        formatter.write_str($expecting)
                    }
                    fn visit_map<A: $crate::__serde::de::MapAccess<'de>>(
                        self,
                        map: A,
                    ) -> ::std::result::Result<$type, A::Error> {
                        <$type>::deserialize(
                            $crate::__serde::de::value::MapAccessDeserializer::new(map),
                        )
                    }
                }
                deserializer.deserialize_map(Map)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(remote = "Self", default, deny_unknown_fields)]
    struct Defaults {
        level: u8,
    }
    crate::map_only!(Defaults, "an object of defaults");

    #[test]
    fn only_an_object_is_read() {
        let read = |value| serde_json::from_value::<Defaults>(value);
        assert_eq!(read(json!({})).unwrap(), Defaults::default());
        assert_eq!(read(json!({ "level": 3 })).unwrap(), Defaults { level: 3 });
        for refused in [json!([]), json!([3]), json!(3), json!(null)] {
            let error = read(refused.clone()).unwrap_err().to_string();
            assert!(
                error.contains("an object of defaults"),
                "{refused}: {error}"
            );
        }
        assert!(read(json!({ "levle": 3 })).is_err(), "still strict");
        assert_eq!(
            serde_json::to_value(Defaults { level: 3 }).unwrap(),
            json!({ "level": 3 })
        );
        let error =
            serde_path_to_error::deserialize::<_, Defaults>(json!({ "level": "x" })).unwrap_err();
        assert_eq!(error.path().to_string(), "level", "the path is kept");
    }
}
