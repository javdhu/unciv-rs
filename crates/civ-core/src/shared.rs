// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Copy-on-write ownership for independently mutable world stores. A staged
//! command shares untouched stores; the first write detaches just that store.
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Shared<T>(Arc<T>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self { Self(self.0.clone()) }
}
impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self { Self(Arc::new(value)) }
}
impl<T: Default> Default for Shared<T> {
    fn default() -> Self { T::default().into() }
}
impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.0 }
}
impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T { Arc::make_mut(&mut self.0) }
}
impl<'a, T> IntoIterator for &'a Shared<T> where &'a T: IntoIterator {
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter { self.deref().into_iter() }
}
impl<'a, T: Clone> IntoIterator for &'a mut Shared<T> where &'a mut T: IntoIterator {
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter { self.deref_mut().into_iter() }
}
