use std::time::{Duration, SystemTime, UNIX_EPOCH};
mod details_loader;

use crate::competitions::{COMPETITIONS_URL, Competition, CompetitionError, parse_competitions};
use chromiumoxide::{
    Browser, Page,
    cdp::browser_protocol::{
        input::InsertTextParams,
        network::{Cookie, CookieParam, CookieSameSite},
        storage::{GetCookiesParams, SetCookiesParams},
        target::{CreateBrowserContextParams, CreateTargetParams},
    },
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::Semaphore,
    time::{sleep, timeout},
};
use url::Url;

pub const ARENA_URL: &str = "https://arena.wodbuster.com/";
const LOGIN_URL: &str = "https://wodbuster.com/account/login.aspx?ReturnUrl=https%3A%2F%2Farena.wodbuster.com%2Fdefault.aspx&cb=arena";

// No Debug: nunca volcar el valor de autenticación a los logs.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: f64,
    pub http_only: bool,
    pub secure: bool,
    pub session: bool,
    pub same_site: Option<String>,
}

impl AuthCookie {
    fn cdp_cookie(&self) -> CookieParam {
        let mut cookie = CookieParam::new(&self.name, &self.value);
        cookie.domain = Some(self.domain.clone());
        cookie.path = Some(self.path.clone());
        cookie.secure = Some(self.secure);
        cookie.http_only = Some(self.http_only);
        cookie.expires = if self.session {
            None
        } else {
            Some(chromiumoxide::cdp::browser_protocol::network::TimeSinceEpoch::new(self.expires))
        };
        cookie.same_site = match self.same_site.as_deref() {
            Some("Strict") => Some(CookieSameSite::Strict),
            Some("Lax") => Some(CookieSameSite::Lax),
            Some("None") => Some(CookieSameSite::None),
            _ => None,
        };
        cookie
    }

    pub fn is_valid(&self) -> bool {
        let domain = self.domain.trim_start_matches('.');
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.name == ".WBAuth"
            && !self.value.is_empty()
            && matches!(domain, "wodbuster.com" | "arena.wodbuster.com")
            && (self.session || self.expires > now)
    }
}

impl From<Cookie> for AuthCookie {
    fn from(cookie: Cookie) -> Self {
        Self {
            name: cookie.name,
            value: cookie.value,
            domain: cookie.domain,
            path: cookie.path,
            expires: cookie.expires,
            http_only: cookie.http_only,
            secure: cookie.secure,
            session: cookie.session,
            same_site: cookie.same_site.map(|site| site.as_ref().to_owned()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("Usuario o contraseña incorrectos")]
    Credentials,
    #[error("WodBuster solicita una verificación adicional")]
    Verification,
    #[error("El acceso ha tardado demasiado")]
    Timeout,
    #[error("No se pudo conectar con el navegador o con WodBuster")]
    Unavailable,
    #[error("Hay otro acceso en curso; inténtalo de nuevo en unos segundos")]
    Busy,
}

pub struct ArenaBrowser {
    endpoint: Url,
    // compose.dev.yml admite dos navegadores simultáneos.
    slots: Semaphore,
}

impl ArenaBrowser {
    pub async fn competitions(
        &self,
        cookie: &AuthCookie,
    ) -> Result<Vec<Competition>, CompetitionError> {
        if !cookie.is_valid() {
            return Err(CompetitionError::SessionExpired);
        }
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| CompetitionError::Busy)?;
        timeout(Duration::from_secs(60), self.competitions_inner(cookie))
            .await
            .map_err(|_| CompetitionError::Timeout)?
    }

    async fn competitions_inner(
        &self,
        cookie: &AuthCookie,
    ) -> Result<Vec<Competition>, CompetitionError> {
        let (browser, mut handler) = Browser::connect(self.endpoint.as_str())
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        let _driver = Driver(tokio::spawn(async move {
            while let Some(result) = handler.next().await {
                if result.is_err() {
                    break;
                }
            }
        }));
        let context = browser
            .create_browser_context(CreateBrowserContextParams {
                dispose_on_detach: Some(true),
                ..Default::default()
            })
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        let result = async {
            browser
                .execute(SetCookiesParams {
                    cookies: vec![cookie.cdp_cookie()],
                    browser_context_id: Some(context.clone()),
                })
                .await
                .map_err(|_| CompetitionError::Unavailable)?;
            let page = browser
                .new_page(
                    CreateTargetParams::builder()
                        .url(COMPETITIONS_URL)
                        .browser_context_id(context.clone())
                        .build()
                        .map_err(|_| CompetitionError::Unavailable)?,
                )
                .await
                .map_err(|_| CompetitionError::Unavailable)?;
            read_competitions(&page).await
        }
        .await;
        let _ = timeout(
            Duration::from_secs(3),
            browser.dispose_browser_context(context),
        )
        .await;
        result
    }

    pub fn new(endpoint: Url) -> Self {
        Self {
            endpoint,
            slots: Semaphore::new(2),
        }
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<AuthCookie, LoginError> {
        let _permit = self.slots.try_acquire().map_err(|_| LoginError::Busy)?;
        timeout(
            Duration::from_secs(90),
            self.login_inner(username, password),
        )
        .await
        .map_err(|_| LoginError::Timeout)?
    }

    async fn login_inner(&self, username: &str, password: &str) -> Result<AuthCookie, LoginError> {
        // Los errores CDP pueden contener URLs o valores introducidos. No se propagan ni registran.
        let (browser, mut handler) = Browser::connect(self.endpoint.as_str())
            .await
            .map_err(|_| LoginError::Unavailable)?;
        let _driver = Driver(tokio::spawn(async move {
            while let Some(result) = handler.next().await {
                if result.is_err() {
                    break;
                }
            }
        }));
        let context = browser
            .create_browser_context(CreateBrowserContextParams {
                dispose_on_detach: Some(true),
                ..Default::default()
            })
            .await
            .map_err(|_| LoginError::Unavailable)?;
        let result = async {
            let page = browser
                .new_page(
                    CreateTargetParams::builder()
                        .url(LOGIN_URL)
                        .browser_context_id(context.clone())
                        .build()
                        .map_err(|_| LoginError::Unavailable)?,
                )
                .await
                .map_err(|_| LoginError::Unavailable)?;
            authenticate(&browser, &page, context.clone(), username, password).await
        }
        .await;
        let _ = timeout(
            Duration::from_secs(3),
            browser.dispose_browser_context(context),
        )
        .await;
        result
    }
}

async fn read_competitions(page: &Page) -> Result<Vec<Competition>, CompetitionError> {
    let mut challenge = false;
    for _ in 0..40 {
        let status = page.evaluate("({authenticated: !!document.querySelector('a[href*=\"signout.ashx\"]'), ready: location.hostname==='arena.wodbuster.com' && location.pathname==='/mywodbusterarena/competitions.aspx' && !!document.querySelector('.competitions'), login: location.hostname==='wodbuster.com' || !!document.querySelector('input[type=password]'), challenge: document.title.includes('Just a moment')})")
            .await.map_err(|_| CompetitionError::Unavailable)?
            .into_value::<serde_json::Value>().map_err(|_| CompetitionError::InvalidPage)?;
        challenge = status["challenge"] == true;
        if status["ready"] == true && status["authenticated"] == true {
            return parse_competitions(
                &page
                    .content()
                    .await
                    .map_err(|_| CompetitionError::Unavailable)?,
            );
        }
        if status["login"] == true && !challenge {
            return Err(CompetitionError::SessionExpired);
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(if challenge {
        CompetitionError::Verification
    } else {
        CompetitionError::InvalidPage
    })
}

async fn authenticate(
    browser: &Browser,
    page: &Page,
    context: chromiumoxide::cdp::browser_protocol::browser::BrowserContextId,
    username: &str,
    password: &str,
) -> Result<AuthCookie, LoginError> {
    wait_for_form(page).await?;
    page.find_element("#body_body_CtlLogin_IoEmail")
        .await
        .map_err(|_| LoginError::Unavailable)?
        .click()
        .await
        .map_err(|_| LoginError::Unavailable)?;
    page.execute(InsertTextParams::new(username))
        .await
        .map_err(|_| LoginError::Unavailable)?;
    page.find_element("#body_body_CtlLogin_IoPassword")
        .await
        .map_err(|_| LoginError::Unavailable)?
        .click()
        .await
        .map_err(|_| LoginError::Unavailable)?;
    page.execute(InsertTextParams::new(password))
        .await
        .map_err(|_| LoginError::Unavailable)?;
    page.find_element("#body_body_CtlLogin_CtlAceptar")
        .await
        .map_err(|_| LoginError::Unavailable)?
        .click()
        .await
        .map_err(|_| LoginError::Unavailable)?;

    let cookie_query = GetCookiesParams {
        browser_context_id: Some(context.clone()),
    };
    let mut device_choice_made = false;
    for attempt in 0..60 {
        if !device_choice_made
            && page
                .find_element("#body_body_CtlConfiar_CtlNoSeguro")
                .await
                .is_ok()
        {
            // El radio puede estar oculto bajo una etiqueta visual. Activa su evento click.
            page.evaluate("document.getElementById('body_body_CtlConfiar_CtlNoSeguro').click()")
                .await
                .map_err(|_| LoginError::Unavailable)?;
            device_choice_made = true;
        }
        let cookies = browser
            .execute(cookie_query.clone())
            .await
            .map_err(|_| LoginError::Unavailable)?
            .result
            .cookies;
        if cookies
            .into_iter()
            .map(AuthCookie::from)
            .any(|cookie| cookie.is_valid())
        {
            // Consultar las cookies desde Arena confirma que son aplicables a ese origen.
            page.goto(ARENA_URL)
                .await
                .map_err(|_| LoginError::Unavailable)?;
            let challenge = page
                .evaluate("document.title.includes('Just a moment')")
                .await
                .map_err(|_| LoginError::Unavailable)?
                .into_value::<bool>()
                .map_err(|_| LoginError::Unavailable)?;
            if challenge {
                return Err(LoginError::Verification);
            }
            let cookie = page
                .get_cookies()
                .await
                .map_err(|_| LoginError::Unavailable)?
                .into_iter()
                .map(AuthCookie::from)
                .find(AuthCookie::is_valid)
                .ok_or(LoginError::Unavailable)?;
            return Ok(cookie);
        }
        if attempt >= 2 {
            let state = page.evaluate("({error: !!document.querySelector('.validation-summary-errors, .field-validation-error, [id*=FailureText], [id*=ErrorMessage]')?.textContent.trim(), challenge: document.title.includes('Just a moment')})")
                        .await.ok().and_then(|value| value.into_value::<serde_json::Value>().ok());
            if state
                .as_ref()
                .is_some_and(|state| state["challenge"] == true)
            {
                return Err(LoginError::Verification);
            }
            if state.as_ref().is_some_and(|state| state["error"] == true) {
                return Err(LoginError::Credentials);
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(LoginError::Timeout)
}

async fn wait_for_form(page: &Page) -> Result<(), LoginError> {
    for _ in 0..40 {
        if page
            .find_element("#body_body_CtlLogin_IoPassword")
            .await
            .is_ok()
        {
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    let challenge = page
        .evaluate("document.title.includes('Just a moment')")
        .await
        .ok()
        .and_then(|result| result.into_value::<bool>().ok())
        .unwrap_or(false);
    Err(if challenge {
        LoginError::Verification
    } else {
        LoginError::Unavailable
    })
}

struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use chromiumoxide::cdp::browser_protocol::fetch::{
        EnableParams, EventRequestPaused, FulfillRequestParams, HeaderEntry,
    };

    #[tokio::test]
    #[ignore = "Requiere Browserless de compose.dev.yml"]
    async fn browserless_login_fixture() -> topcoat::Result<()> {
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env()?;
        let (browser, mut handler) = Browser::connect(config.browser_url.as_str()).await?;
        let _driver = Driver(tokio::spawn(async move {
            while handler.next().await.is_some() {}
        }));
        // Las respuestas del sitio se simulan; Chromium y CDP son los servicios reales.
        for accepted in [true, false] {
            let context = browser
                .create_browser_context(CreateBrowserContextParams {
                    dispose_on_detach: Some(true),
                    ..Default::default()
                })
                .await?;
            let page = browser
                .new_page(
                    CreateTargetParams::builder()
                        .url("about:blank")
                        .browser_context_id(context.clone())
                        .build()
                        .unwrap(),
                )
                .await?;
            let mut requests = page.event_listener::<EventRequestPaused>().await?;
            page.execute(EnableParams::default()).await?;
            let intercepted = page.clone();
            let _fixture = Driver(tokio::spawn(async move {
                while let Some(event) = requests.next().await {
                    let mut headers =
                        vec![HeaderEntry::new("Content-Type", "text/html; charset=utf-8")];
                    let html = if event.request.method == "POST" {
                        let data = event
                            .request
                            .post_data_entries
                            .as_ref()
                            .unwrap()
                            .iter()
                            .filter_map(|entry| entry.bytes.as_ref())
                            .flat_map(|entry| {
                                base64::engine::general_purpose::STANDARD
                                    .decode(entry)
                                    .unwrap()
                            })
                            .collect::<Vec<_>>();
                        let fields = url::form_urlencoded::parse(&data)
                            .collect::<std::collections::HashMap<_, _>>();
                        if fields.contains_key("noRemember") {
                            assert!(accepted);
                            headers.push(HeaderEntry::new("Set-Cookie", ".WBAuth=fixture-auth-cookie; Domain=.wodbuster.com; Path=/; Secure; HttpOnly; SameSite=Lax"));
                            "<html><title>Arena</title><body>Autenticado</body></html>"
                        } else {
                            assert_eq!(
                                fields.get("email").map(|s| s.as_ref()),
                                Some("fixture@example.com")
                            );
                            assert_eq!(
                                fields.get("password").map(|s| s.as_ref()),
                                Some("contraseña&+= prueba")
                            );
                            assert!(
                                fields.contains_key("accept"),
                                "Debe pulsar el submit visible"
                            );
                            if accepted {
                                "<html><body><form method=post><input id=body_body_CtlConfiar_CtlNoSeguro type=radio name=noRemember onclick='this.form.submit()'></form></body></html>"
                            } else {
                                "<html><body><div class=validation-summary-errors>Credenciales incorrectas</div></body></html>"
                            }
                        }
                    } else if event.request.url.contains("account/login") {
                        // El formulario real tiene un submit oculto antes del botón Aceptar.
                        "<html><body><form method=post><input type=submit name=hidden style='display:none'><input id=body_body_CtlLogin_IoEmail type=email name=email><input id=body_body_CtlLogin_IoPassword type=password name=password><input id=body_body_CtlLogin_CtlAceptar type=submit name=accept value=Aceptar></form></body></html>"
                    } else {
                        "<html><title>Arena</title><body>Autenticado</body></html>"
                    };
                    let mut response = FulfillRequestParams::new(event.request_id.clone(), 200);
                    response.response_headers = Some(headers);
                    response.body = Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(html)
                            .into(),
                    );
                    intercepted.execute(response).await.unwrap();
                }
            }));
            page.goto(LOGIN_URL).await?;
            let result = timeout(
                Duration::from_secs(45),
                authenticate(
                    &browser,
                    &page,
                    context.clone(),
                    "fixture@example.com",
                    "contraseña&+= prueba",
                ),
            )
            .await?;
            if accepted {
                let cookie = result?;
                assert!(cookie.value == "fixture-auth-cookie");
                assert!(cookie.http_only && cookie.secure && cookie.session);
                assert!(cookie.domain == ".wodbuster.com" && cookie.path == "/");
                assert!(cookie.same_site.as_deref() == Some("Lax"));
            } else {
                assert!(matches!(result, Err(LoginError::Credentials)));
            }
            browser.dispose_browser_context(context).await?;
        }
        Ok(())
    }
}
