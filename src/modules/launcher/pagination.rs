use crate::gtk_helpers::IronbarGtkExt;
use crate::image::new_icon_button;
use gtk::prelude::*;
use gtk::{Button, IconTheme, Orientation};
use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

pub struct Pagination {
    offset: Rc<RefCell<usize>>,

    controls_container: gtk::Box,
    btn_fwd: Button,
}

pub struct IconContext<'a> {
    pub icon_back: &'a str,
    pub icon_fwd: &'a str,
    pub icon_size: i32,
    pub icon_theme: &'a IconTheme,
}

impl Pagination {
    pub fn new(
        container: &gtk::Box,
        page_size: usize,
        orientation: Orientation,
        icon_context: IconContext,
    ) -> Self {
        let scroll_box = gtk::Box::new(orientation, 0);

        let scroll_back = new_icon_button(
            icon_context.icon_back,
            icon_context.icon_theme,
            icon_context.icon_size,
        );

        let scroll_fwd = new_icon_button(
            icon_context.icon_fwd,
            icon_context.icon_theme,
            icon_context.icon_size,
        );

        scroll_back.set_sensitive(false);
        scroll_fwd.set_sensitive(false);

        scroll_box.add_class("pagination");
        scroll_back.add_class("btn-back");
        scroll_fwd.add_class("btn-forward");

        scroll_box.add(&scroll_back);
        scroll_box.add(&scroll_fwd);
        container.add(&scroll_box);

        let offset = Rc::new(RefCell::new(1));

        {
            let offset = offset.clone();
            let container = container.clone();
            let scroll_back = scroll_back.clone();

            scroll_fwd.connect_clicked(move |btn| {
                let mut offset = offset.borrow_mut();
                let child_count = container.children().len();

                *offset = std::cmp::min(child_count - 1, *offset + page_size);

                Self::update_page(&container, *offset, page_size);

                if *offset + page_size >= child_count {
                    btn.set_sensitive(false);
                }

                scroll_back.set_sensitive(true);
            });
        }

        {
            let offset = offset.clone();
            let container = container.clone();
            let scroll_fwd = scroll_fwd.clone();

            scroll_back.connect_clicked(move |btn| {
                let mut offset = offset.borrow_mut();
                // avoid using std::cmp::max due to possible overflow
                if page_size < *offset {
                    *offset -= page_size;
                } else {
                    *offset = 1
                };

                Self::update_page(&container, *offset, page_size);

                if *offset == 1 || *offset - page_size < 1 {
                    btn.set_sensitive(false);
                }

                scroll_fwd.set_sensitive(true);
            });
        }

        Self {
            offset,

            controls_container: scroll_box,
            btn_fwd: scroll_fwd,
        }
    }

    fn update_page(container: &gtk::Box, offset: usize, page_size: usize) {
        for (i, btn) in container.children().iter().enumerate() {
            // skip offset buttons
            if i == 0 {
                continue;
            }

            if i >= offset && i < offset + page_size {
                btn.show();
            } else {
                btn.hide();
            }
        }
    }

    pub fn set_sensitive_fwd(&self, sensitive: bool) {
        self.btn_fwd.set_sensitive(sensitive);
    }

    pub fn offset(&self) -> usize {
        *self.offset.borrow()
    }
}

impl Deref for Pagination {
    type Target = gtk::Box;

    fn deref(&self) -> &Self::Target {
        &self.controls_container
    }
}
