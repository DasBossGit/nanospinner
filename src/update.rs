pub trait Callback<State>: Fn(&mut State) -> String + Send + 'static {}
impl<State, F> Callback<State> for F where F: Fn(&mut State) -> String + Send + 'static {}

#[doc(hidden)]
pub enum UpdateStrategy<State> {
    Message(String),
    Callback {
        state: State,
        callback: Box<dyn Callback<State>>,
    },
}

macro_rules! impl_from {
    ($($t:ty $(: {$($method:tt)+})?),*) => {
        $(
            impl<State> From<$t> for UpdateStrategy<State> {
                fn from(message: $t) -> Self {
                    UpdateStrategy::Message(message $($($method)+)?.into())
                }
            }
        )*
    };
}

impl_from! { String, &str, ::std::borrow::Cow<'_,str> , ::std::sync::Arc<str> : {.as_ref()}, Box<str>, ::std::rc::Rc<str>: {.as_ref()} }

impl<State> ::std::str::FromStr for UpdateStrategy<State> {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UpdateStrategy::Message(s.to_string()))
    }
}

impl<State, F> From<F> for UpdateStrategy<State>
where
    F: Callback<State>,
    State: Send + Default,
{
    fn from(callback: F) -> Self {
        UpdateStrategy::Callback {
            state: State::default(),
            callback: Box::new(callback),
        }
    }
}
impl<State, F> From<(F, State)> for UpdateStrategy<State>
where
    F: Callback<State>,
    State: Send,
{
    fn from((callback, state): (F, State)) -> Self {
        UpdateStrategy::Callback {
            state,
            callback: Box::new(callback),
        }
    }
}

impl<State> UpdateStrategy<State> {
    pub fn new_message(message: impl Into<String>) -> Self {
        UpdateStrategy::Message(message.into())
    }
    pub fn new_callback(callback: impl Callback<State>, state: State) -> Self {
        UpdateStrategy::Callback {
            state,
            callback: Box::new(callback),
        }
    }
}
