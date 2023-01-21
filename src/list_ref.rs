use std::marker::PhantomData;
use crate::{
    StackShared,
    prim_ref::{
        PrimRef,
        PrimExpr,
    },
    rec_ref::{
        ListToRecMappable,
    },
};

pub trait ToListMappable {
    type O;

    fn do_map(self, base: String) -> Self::O;
}

pub trait RecToListMappable {
    type O;

    fn do_map(self, base: String) -> Self::O;
}

pub struct ListRef<T> {
    pub(crate) shared: StackShared,
    pub(crate) base: String,
    _pd: PhantomData<T>,
}

impl<T> PrimRef for ListRef<T> {
    fn new(shared: StackShared, base: String) -> Self {
        ListRef {
            shared: shared,
            base: base,
            _pd: Default::default(),
        }
    }
}

impl<T: PrimRef> ListRef<T> {
    pub fn get(&self, index: usize) -> T {
        T::new(self.shared.clone(), format!("{}[{}]", &self.base, index))
    }

    pub fn map<O: ToListMappable>(&self, inner: impl FnOnce(T) -> O) -> O::O {
        let out = inner(T::new(self.shared.clone(), "each.value".into()));
        out.do_map(self.base.clone())
    }

    pub fn map_obj<O: ListToRecMappable>(&self, inner: impl FnOnce(T) -> (PrimExpr<String>, O)) -> O::O {
        let (k, out) = inner(T::new(self.shared.clone(), "each.value".into()));
        out.do_map_obj(self.base.clone(), k)
    }
}

pub struct MapListRef<T> {
    pub(crate) shared: StackShared,
    pub(crate) base: String,
    pub(crate) map_base: String,
    _pd: PhantomData<T>,
}

impl<T> MapListRef<T> {
    pub(crate) fn new(shared: StackShared, base: String, map_base: String) -> Self {
        MapListRef {
            shared: shared,
            base: base,
            map_base: map_base,
            _pd: Default::default(),
        }
    }
}

impl<T: PrimRef> MapListRef<T> {
    pub fn map<O: ToListMappable>(&self, inner: impl FnOnce(T) -> O) -> O::O {
        let out = inner(T::new(self.shared.clone(), self.map_base.to_string()));
        out.do_map(self.base.clone())
    }
}

pub struct MapListRefToRec<T> {
    pub(crate) shared: StackShared,
    pub(crate) base: String,
    pub(crate) map_base_key: String,
    pub(crate) map_base: String,
    _pd: PhantomData<T>,
}

impl<T> MapListRefToRec<T> {
    pub(crate) fn new(shared: StackShared, base: String, map_base_key: String, map_base: String) -> Self {
        MapListRefToRec {
            shared: shared,
            base: base,
            map_base_key: map_base_key,
            map_base: map_base,
            _pd: Default::default(),
        }
    }
}
