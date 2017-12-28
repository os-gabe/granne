#[macro_use]
extern crate cpython;

extern crate memmap;
extern crate hnsw;

use cpython::{PyObject, PyResult};
use std::cell::RefCell;
use std::fs::File;
use memmap::Mmap;

py_module_initializer!(hnsw, inithnsw, PyInit_hnsw, |py, m| {
    try!(m.add(py, "__doc__", "This module is implemented in Rust."));
    try!(m.add_class::<HnswBuilder>(py));

    Ok(())
});


py_class!(class HnswBuilder |py| {
    data builder: RefCell<hnsw::HnswBuilder<hnsw::NormalizedFloatElement>>;

    def __new__(_cls,
                num_layers: usize,
                max_search: usize = 50,
                show_progress: bool = true) -> PyResult<HnswBuilder> {

        let config = hnsw::Config {
            num_layers: num_layers,
            max_search: max_search,
            show_progress: show_progress
        };

        let builder = hnsw::HnswBuilder::new(config);

        HnswBuilder::create_instance(py, RefCell::new(builder))
    }

    @classmethod def load(_cls, path: &str) -> PyResult<HnswBuilder> {
        let file = File::open(path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };
        let index = hnsw::Hnsw::load(&mmap);

        let config = hnsw::Config {
            num_layers: 1,
            max_search: 50,
            show_progress: true,
        };

        let builder = hnsw::HnswBuilder::from_index(config, &index);

        HnswBuilder::create_instance(py, RefCell::new(builder))
    }

    def add(&self, element: Vec<f32>) -> PyResult<PyObject> {
        self.builder(py).borrow_mut().add(vec![convert_to_element(element)]);

        Ok(py.None())
    }

    def __len__(&self) -> PyResult<usize> {
        Ok(self.builder(py).borrow().get_index().len())
    }

    def build(&self) -> PyResult<PyObject> {
        self.builder(py).borrow_mut().build_index();

        Ok(py.None())
    }

    def save(&self, path: &str) -> PyResult<PyObject> {
        self.builder(py).borrow().save_to_disk(path);

        Ok(py.None())
    }

    def search(&self, element: Vec<f32>,
               num_elements: usize = 5,
               max_search: usize = 50) -> PyResult<Vec<(usize, f32)>>
    {
        let builder = self.builder(py).borrow();
        let search_index = builder.get_index();

        Ok(search_index.search(
            &convert_to_element(element), num_elements, max_search))
    }

});


fn convert_to_element(element: Vec<f32>) -> hnsw::NormalizedFloatElement {
    assert_eq!(hnsw::DIM, element.len());

    let mut data = [0.0f32; hnsw::DIM];
    data.copy_from_slice(element.as_slice());

    hnsw::FloatElement::from(data).normalized()
}