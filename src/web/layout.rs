use std::sync::Mutex;

use topcoat::{
    Result,
    context::{Cx, memoize},
    router::{Slot, layout},
    view::{View, ViewExt, component, view},
};

#[derive(Clone, Copy)]
pub(super) struct Back {
    target: &'static str,
    label: &'static str,
}

impl Back {
    pub(super) const HOME: Self = Self {
        target: "/",
        label: "Volver",
    };
    pub(super) const COMPETITIONS: Self = Self {
        target: "/competitions",
        label: "Volver a Mis competiciones",
    };
}

#[derive(Clone, Default)]
struct PageOptions {
    title: String,
    wide: bool,
    back: Option<Back>,
}

// Topcoat comparte este valor únicamente entre las vistas de la misma petición.
#[memoize]
fn page_options(cx: &Cx) -> Mutex<PageOptions> {
    Mutex::new(PageOptions::default())
}

pub(super) fn configure(cx: &Cx, title: impl Into<String>, wide: bool, back: Option<Back>) {
    *page_options(cx).lock().expect("page options lock") = PageOptions {
        title: title.into(),
        wide,
        back,
    };
}

#[layout("/")]
pub(super) async fn app_layout(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    // La página define su título antes de renderizar la cabecera del documento.
    let content = slot.single().await?;
    let options = page_options(cx).lock().expect("page options lock").clone();
    let title = if options.title.is_empty() {
        "WodBuster Jueces".to_owned()
    } else {
        format!("{} · WodBuster Jueces", options.title)
    };
    Ok(view! {
        <!DOCTYPE html>
        <html lang="es">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <style>(include_str!("../style.css"))</style>
            </head>
            <body>
                <main class=(options.wide.then_some("competition-detail"))>
                    <p class="brand">"WODBUSTER / JUECES"</p>
                    <section class="card" aria-labelledby="title">
                        if let Some(back) = options.back { back_button(back: back, top: true) }
                        (content)
                        if let Some(back) = options.back { back_button(back: back) }
                    </section>
                </main>
            </body>
        </html>
    })
}

#[component]
async fn back_button(back: Back, #[default] top: bool) -> Result<impl View> {
    Ok(view! {
        <form method="get" action=(back.target) class=(top.then_some("top-navigation"))>
            <button type="submit" class="secondary">(back.label)</button>
        </form>
    })
}
