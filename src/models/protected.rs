//! The "Protected" model builds off the Model object to provide a set of tools
//! for handling data safely:
//!
//! - Separation of public and private fields in a model. Essentially, this
//! means fields that will be outside of an encrypted `body` field when
//! serialized (public) and fields that will be *inside* the encrypted `body`
//! field when serialized (private).
//! - (De)serialization. Serializing a protected model means taking its private
//! fields, stringifying them via JSON, and encrypting the resulting string into
//! a `body` field. Deserializing a protected model means reading the `body`
//! field from data, decrypting it, and updating its data with the values from
//! inside the JSON dump. Note that for both operations, the model needs to have
//! a `key` set, which is used as the key for cryptographic operations.
//! - Finding a matching key for an object either from a sibling/parent object
//! or from the current user's keychain.
//!
//! This is mostly provided through the use of a `Protected` trait and a
//! `protected! {} macro, used to wrap around struct definitions to make them
//! protected. This macro also implements the `Debug` trait for the defined
//! models so they don't go around spraying their private fields into debug
//! logs.

use std::collections::BTreeMap;

use ::std::fmt;
use ::jedi::{self, Value};

use ::error::{TResult, TError};
use ::models::model::Model;
use ::crypto::{self, CryptoOp};

/// The Protected trait defines a set of functionality for our models such that
/// they are able to be properly (de)serialized (including encryption/decryption
/// of the model).
///
/// It also defines methods that make it easy to do The Right Thing (c)(r)(tm)
/// when handling protected model data. The goal here is to eliminate all forms
/// of data leaks while providing an interface that's easy to use.
pub trait Protected: Model + fmt::Debug {
    /// Get the key for this model
    fn key(&self) -> Option<&Vec<u8>>;

    /// Set this model's key
    fn set_key(&mut self, key: Option<Vec<u8>>);

    /// Get this model's "type" (ie, "note", "board", etc).
    fn model_type(&self) -> &str;

    /// Grab the public fields for this model
    fn public_fields(&self) -> Vec<&'static str>;

    /// Grab the private fields for this model
    fn private_fields(&self) -> Vec<&'static str>;

    /// Grab the fields names of any child models this model has
    fn submodel_fields(&self) -> Vec<&'static str>;

    /// Get (JSON) data from one of our submodels
    fn submodel_data(&self, field: &str, private: bool) -> TResult<Value>;

    /// Sets our key into all our submodels
    fn _set_key_on_submodels(&mut self);

    /// Serializes our submodels
    fn serialize_submodels(&mut self) -> TResult<()>;

    /// Deserializes our submodels
    fn deserialize_submodels(&mut self) -> TResult<()>;

    /// Like Model::set_multi(), but sets data into submodels
    fn set_multi_recursive(&mut self, data: ::jedi::Value) -> TResult<()>;

    /// Grab the name of this model's table
    fn table(&self) -> String;

    /// Either grab the existing or generate a new key for this model
    fn generate_key(&mut self) -> TResult<&Vec<u8>>;

    /// Get the model's body data
    fn get_body<'a>(&'a self) -> Option<&'a String>;

    /// Set the model's body data
    fn set_body(&mut self, body: String);

    /// Grab a JSON Value representation of ALL this model's data
    fn data(&self) -> Value {
        jedi::to_val(self)
    }

    /// Get a set of fields and return them as a JSON Value
    fn get_fields(&self, fields: &Vec<&str>) -> BTreeMap<String, Value> {
        let mut map: BTreeMap<String, jedi::Value> = BTreeMap::new();
        let data = jedi::to_val(self);
        for field in fields {
            let val = jedi::walk(&[field], &data);
            match val {
                Ok(v) => { map.insert(String::from(*field), v.clone()); },
                Err(..) => {}
            }
        }
        map
    }

    /// Get a set of fields and return them as a JSON Value
    fn get_serializable_data(&self, private: bool) -> Value {
        let fields = if private {
            self.private_fields()
        } else {
            self.public_fields()
        };
        let mut map = self.get_fields(&fields);
        let submodels = self.submodel_fields();
        // shove in our submodels' public/private data
        for field in submodels {
            let val: TResult<Value> = self.submodel_data(field, private);
            match val {
                Ok(v) => { map.insert(String::from(field), v); },
                Err(..) => {},
            }
        }
        Value::Object(map)
    }

    /// Grab all public fields for this model as a json Value
    ///
    /// NOTE: Don't use this directly. Use `data_for_storage()` instead!
    /// TODO: prefix with _
    fn _public_data(&self) -> Value {
        self.get_serializable_data(false)
    }

    /// Grab all private fields for this model as a json Value
    ///
    /// NOTE: Don't use this directly. Use `data()` instead!
    /// TODO: prefix with _
    fn _private_data(&self) -> Value {
        self.get_serializable_data(true)
    }

    /// Grab all public fields for this model as a JSON Value.
    fn data_for_storage(&self) -> Value {
        self._public_data()
    }

    /// Return a JSON dump of all fields. Really, this is a wrapper around
    /// `jedi::stringify(model.data())`.
    ///
    /// Use this function when sending a model to a trusted source (ie inproc
    /// messaging to our view layer).
    ///
    /// __NEVER__ use this function to save data to disk or transmit over a
    /// network connection.
    fn stringify_unsafe(&self) -> TResult<String> {
        jedi::stringify(&self.data()).map_err(|e| toterr!(e))
    }

    /// Return a JSON dump of all public fields. Really, this is a wrapper
    /// around `jedi::stringify(model.data_for_storage())`.
    ///
    /// Use this function for sending a model to an *untrusted* source, such as
    /// saving to disk or over a network connection.
    fn stringify_for_storage(&self) -> TResult<String> {
        jedi::stringify(&self.data_for_storage()).map_err(|e| toterr!(e))
    }

    /// "Serializes" a model...returns all public data with an *encrypted* set
    /// of private data (in `body`).
    ///
    /// It returns the Value of all *public* fields, but with the `body`
    /// populated with the encrypted data.
    fn serialize(&mut self) -> TResult<Value> {
        try!(self.serialize_submodels());
        let body;
        {
            let fakeid = String::from("<no id>");
            let id = match self.id() {
                Some(x) => x,
                None => &fakeid,
            };
            let data = self._private_data();
            let json = try!(jedi::stringify(&data));

            let key = match self.key() {
                Some(x) => x,
                None => return Err(TError::BadValue(format!("Protected::serialize() - missing `key` field for {} model {}", self.model_type(), id))),
            };
            body = try!(crypto::encrypt(&key, Vec::from(json.as_bytes()), try!(CryptoOp::new("aes", "gcm"))));
        }
        let body_base64 = try!(crypto::to_base64(&body));
        self.set_body(body_base64);
        Ok(self.data_for_storage())
    }

    /// "DeSerializes" a model...takes the `body` field, decrypts it, and sets
    /// the values in the decrypted JSON dump back into the model.
    ///
    /// It returns the Value of all public fields.
    fn deserialize(&mut self) -> TResult<Value> {
        try!(self.deserialize_submodels());
        let fakeid = String::from("<no id>");
        let json_bytes;
        {
            let id = match self.id() {
                Some(x) => x,
                None => &fakeid,
            };
            let body = match self.get_body() {
                Some(x) => try!(crypto::from_base64(x)),
                None => return Err(TError::MissingField(format!("Protected::deserialize() - missing `body` field for {} model {}", self.model_type(), id))),
            };
            let key = match self.key() {
                Some(x) => x,
                None => return Err(TError::BadValue(format!("Protected::deserialize() - missing `key` field for {} model {}", self.model_type(), id))),
            };
            json_bytes = try!(crypto::decrypt(&key, &body));
        }
        let json_str = try!(String::from_utf8(json_bytes));
        let parsed: Value = try!(jedi::parse(&json_str));
        try!(self.set_multi_recursive(parsed));
        Ok(self.data())
    }

    fn ensure_key(&mut self) -> Option<&Vec<u8>> {
        let key = self.key();
        key
    }
}

/// Defines a protected model for us. We give it a model name, a set of public
/// fields, a set of private fields, and lastly a set of extra fields (neither
/// public nor private) and it defines our model struct, and implements the
/// Protected trait for us, as well as a handy debug trait (that won't leak
/// private information on print).
///
/// NOTE that the `id` and `body` fields are always prepended to the public
/// field list as `id: String` and `body: String` so don't include the id/body
/// fields in your public/private field lists. OR ELSE.
///
/// # Examples
///
/// ```
/// # #[macro_use] mod models;
/// # fn main() {
/// protected!(Squirrel, (size: i64), (name: String), ());
/// # }
/// ```
#[macro_export]
macro_rules! protected {
    (
        $(#[$struct_meta:meta])*
        pub struct $name:ident {
            ( $( $pub_field:ident: $pub_type:ty ),* ),
            ( $( $priv_field:ident: $priv_type:ty ),* ),
            ( $( $extra_field:ident: $extra_type:ty ),* )
        }
    ) => {
        protected! {
            $(#[$struct_meta])*
            pub struct $name {
                ( $( $pub_field: $pub_type ),* ),
                ( $( $priv_field: $priv_type ),* ),
                ( $( $extra_field: $extra_type ),* ),
                ( )
            }
        }
    };

    // struct implementation
    (
        $(#[$struct_meta:meta])*
        pub struct $name:ident {
            ( $( $pub_field:ident: $pub_type:ty ),* ),
            ( $( $priv_field:ident: $priv_type:ty ),* ),
            ( $( $extra_field:ident: $extra_type:ty ),* ),
            ( $( $submodel_field:ident ),* )
        }
    ) => {
        // define the struct
        model! {
            $(#[$struct_meta])*
            pub struct $name {
                (
                    $( $extra_field: $extra_type, )*
                    _key: Option<Vec<u8>>,
                    model_type: String
                )

                $( $pub_field: $pub_type, )*
                $( $priv_field: $priv_type, )*
                body: String, 
            }
        }

        // run our implementations
        protected!([IMPL ( $name ), ( $( $pub_field ),* ), ( $( $priv_field ),* ), ( $( $extra_field ),* ), ( $( $submodel_field ),* )]);
    };

    // protected implementation
    (
        [IMPL ( $name:ident ),
              ( $( $pub_field:ident ),* ),
              ( $( $priv_field:ident ),* ),
              ( $( $extra_field:ident ),* ),
              ( $( $submodel_field:ident ),* ) ]

    ) => {
        // make sure printing out a model doesn't leak data
        impl ::std::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                let fakeid = String::from("<no id>");
                let id = match self.id() {
                    Some(x) => x,
                    None => &fakeid,
                };
                write!(f, "{}: ({})", self.model_type(), id)
            }
        }

        impl Protected for $name {
            fn key(&self) -> Option<&Vec<u8>> {
                match self._key.as_ref() {
                    Some(x) => Some(x),
                    None => None,
                }
            }

            fn set_key(&mut self, key: Option<Vec<u8>>) {
                self._key = key;
                self._set_key_on_submodels();
            }

            fn model_type(&self) -> &str {
                &self.model_type[..]
            }

            fn public_fields(&self) -> Vec<&'static str> {
                vec![
                    "id",
                    "body",
                    $( fix_type!(stringify!($pub_field)), )*
                ]
            }

            fn private_fields(&self) -> Vec<&'static str> {
                vec![
                    $( fix_type!(stringify!($priv_field)), )*
                ]
            }

            fn submodel_fields(&self) -> Vec<&'static str> {
                vec![
                    $( fix_type!(stringify!($submodel_field)), )*
                ]
            }

            #[allow(unused_variables)]  // required in case we have no submodels
            fn submodel_data(&self, field: &str, private: bool) -> ::error::TResult<::jedi::Value> {
                $(
                    if field == fix_type!(stringify!($submodel_field)) {
                        match self.$submodel_field.as_ref() {
                            Some(ref x) => {
                                return Ok(x.get_serializable_data(private));
                            },
                            None => return Ok(::jedi::Value::Null),
                        }
                    }
                )*
                Err(::error::TError::MissingField(format!("The field {} wasn't found in this model", field)))
            }

            fn _set_key_on_submodels(&mut self) {
                if self.key().is_none() { return; }
                $(
                    {
                        let key = self.key().unwrap().clone();
                        match self.$submodel_field.as_mut() {
                            Some(ref mut x) => x.set_key(Some(key)),
                            None => {},
                        }
                    }
                )*
            }

            fn serialize_submodels(&mut self) -> ::error::TResult<()> {
                $(
                    match self.$submodel_field.as_mut() {
                        Some(ref mut x) => {
                            try!(x.serialize());
                        },
                        None => {},
                    }
                )*
                Ok(())
            }

            fn deserialize_submodels(&mut self) -> ::error::TResult<()> {
                $(
                    match self.$submodel_field.as_mut() {
                        Some(ref mut x) => {
                            try!(x.deserialize());
                        },
                        None => {},
                    }
                )*
                Ok(())
            }

            // override model::Model to handle submodels
            #[allow(unused_mut)]
            fn set_multi_recursive(&mut self, data: ::jedi::Value) -> ::error::TResult<()> {
                let mut hash = match data {
                    ::jedi::Value::Object(x) => x,
                    _ => return Err(::error::TError::BadValue(String::from("protected.set_multi() -- invalid JSON object"))),
                };
                $(
                    match hash.remove(&String::from(stringify!($submodel_field))) {
                        Some(x) => {
                            if self.$submodel_field.is_none() {
                                // a bit hacky, but honestly not sure how else to get a
                                // new instance
                                self.$submodel_field = Some(::jedi::parse(&String::from("{}")).unwrap());
                            }
                            try!(self.$submodel_field.as_mut().unwrap().set_multi(x));
                        },
                        None => {},
                    }
                )*
                self.set_multi(::jedi::Value::Object(hash))
            }

            // TODO: change to &'static str?? why is this a string??
            fn table(&self) -> String {
                String::from(stringify!($name)).to_lowercase()
            }

            fn generate_key(&mut self) -> ::error::TResult<&Vec<u8>> {
                if !self.key().is_some() {
                    let key = try!(::crypto::random_key());
                    self.set_key(Some(key));
                }
                Ok(self.key().unwrap())
            }

            fn get_body<'a>(&'a self) -> Option<&'a String> {
                match self.body {
                    Some(ref x) => Some(x),
                    None => None,
                }
            }

            fn set_body(&mut self, body: String) {
                self.body = Some(body);
            }
        }
    }
}

/// Defines a key struct, used by many models that have subkey data.
protected!{
    pub struct Key {
        (), (), ()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::jedi;
    use ::crypto;
    use ::models::model::Model;

    protected!{
        pub struct Dog {
            ( size: i64 ),
            ( name: String,
              type_: String,
              tags: Vec<String> ),
            ( active: bool )
        }
    }

    protected!{
        pub struct Junkyard {
            ( name: String ),
            // Uhhh, I'm sorry. Is this not a junkyard?!
            ( dog: Dog ),
            ( ),
            ( dog )
        }
    }

    #[test]
    fn returns_correct_public_fields() {
        let dog = Dog::new();
        assert_eq!(dog.public_fields(), ["id", "body", "size"]);
    }

    #[test]
    fn returns_correct_private_fields() {
        let dog = Dog::new();
        assert_eq!(dog.private_fields(), ["name", "type", "tags"]);
    }

    #[test]
    fn handles_public_data() {
        let mut dog = Dog::new();
        dog.active = true;
        dog.id = Some(String::from("123"));
        dog.size = Some(42i64);
        dog.name = Some(String::from("barky"));
        assert_eq!(jedi::stringify(&dog.data_for_storage()).unwrap(), r#"{"body":null,"id":"123","size":42}"#);
        assert_eq!(dog.stringify_for_storage().unwrap(), r#"{"body":null,"id":"123","size":42}"#);
    }

    #[test]
    fn can_serialize_json() {
        let mut dog = Dog::new();
        dog.size = Some(32i64);
        dog.name = Some(String::from("timmy"));
        dog.type_ = Some(String::from("tiny"));
        dog.tags = Some(vec![String::from("canine"), String::from("3-legged")]);
        // tests for presence of `extra` fields in JSON (there should be none)
        dog.active = true;
        assert_eq!(dog.stringify_unsafe().unwrap(), r#"{"body":null,"id":null,"name":"timmy","size":32,"tags":["canine","3-legged"],"type":"tiny"}"#);
        {
            let mut tags: &mut Vec<String> = dog.tags.as_mut().unwrap();
            tags.push(String::from("fast"));
        }
        assert_eq!(dog.stringify_unsafe().unwrap(), r#"{"body":null,"id":null,"name":"timmy","size":32,"tags":["canine","3-legged","fast"],"type":"tiny"}"#);
    }

    #[test]
    fn encrypts_decrypts() {
        let json = String::from(r#"{"size":69,"name":"barky","type":"canadian","tags":["flappy","noisy"]}"#);
        let mut dog: Dog = jedi::parse(&json).unwrap();
        let key = crypto::random_key().unwrap();
        dog.set_key(Some(key.clone()));
        let serialized = dog.serialize().unwrap();

        let body: String = jedi::get(&["body"], &serialized).unwrap();
        match jedi::get::<String>(&["name"], &serialized) {
            Ok(..) => panic!("data from Protected::serialize() contains private fields"),
            Err(e) => match e {
                jedi::JSONError::NotFound(..) => (),
                _ => panic!("error while testing data returned from Protected::serialize() - {}", e),
            }
        }
        assert_eq!(&body, dog.body.as_ref().unwrap());

        let mut dog2 = Dog::new();
        dog2.set_multi(dog.data_for_storage()).unwrap();
        assert_eq!(dog.stringify_for_storage().unwrap(), dog2.stringify_for_storage().unwrap());
        dog2.set_key(Some(key.clone()));
        assert_eq!(dog2.size.unwrap(), 69);
        assert_eq!(dog2.name, None);
        assert_eq!(dog2.type_, None);
        assert_eq!(dog2.tags, None);
        let res = dog2.deserialize().unwrap();
        assert_eq!(dog.stringify_unsafe().unwrap(), dog2.stringify_unsafe().unwrap());
        assert_eq!(jedi::get::<String>(&["name"], &res).unwrap(), "barky");
        assert_eq!(jedi::get::<String>(&["type"], &res).unwrap(), "canadian");
        assert_eq!(dog2.size.unwrap(), 69);
        assert_eq!(dog2.name.unwrap(), String::from("barky"));
        assert_eq!(dog2.type_.unwrap(), String::from("canadian"));
        assert_eq!(dog2.tags.unwrap(), vec!["flappy", "noisy"]);
    }

    #[test]
    fn recursive_serialization() {
        let mut junkyard: Junkyard = jedi::parse(&String::from(r#"{"name":"US political system","dog":{"size":69,"name":"Gerard","type":"chowchow","tags":["bites","stubborn","furry"]}}"#)).unwrap();
        assert_eq!(junkyard.stringify_for_storage().unwrap(), String::from(r#"{"body":null,"dog":{"body":null,"id":null,"size":69},"id":null,"name":"US political system"}"#));
        assert_eq!(junkyard.stringify_unsafe().unwrap(), String::from(r#"{"body":null,"dog":{"body":null,"id":null,"name":"Gerard","size":69,"tags":["bites","stubborn","furry"],"type":"chowchow"},"id":null,"name":"US political system"}"#));
        junkyard.generate_key().unwrap();
        junkyard.serialize().unwrap();

        // ok, we serialized some stuff, let's see if we did it recursively AND
        // if we can undo it
        let storage = junkyard.stringify_for_storage().unwrap();

        let mut junkyard2: Junkyard = jedi::parse(&storage).unwrap();
        assert_eq!(junkyard2.dog.as_ref().unwrap().size.as_ref().unwrap(), &69);
        junkyard2.set_key(Some(junkyard.key().unwrap().clone()));
        junkyard2.deserialize().unwrap();
        assert_eq!(junkyard2.dog.as_ref().unwrap().size.as_ref().unwrap(), &69);
        let mut dog = junkyard2.dog.as_mut().unwrap();
        assert_eq!(dog.name.as_ref().unwrap(), &String::from("Gerard"));
        assert_eq!(dog.type_.as_ref().unwrap(), &String::from("chowchow"));
        assert_eq!(dog.size.as_ref().unwrap(), &69);
        dog.body = None;
        assert_eq!(dog.stringify_unsafe().unwrap(), String::from(r#"{"body":null,"id":null,"name":"Gerard","size":69,"tags":["bites","stubborn","furry"],"type":"chowchow"}"#));
    }
}

