//! First-run host selection, explicit network interface and saved favorites.
use super::theme;
use crate::app::LanvoxApp;
use egui::{Frame, Margin, RichText};
pub fn show(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().show(ui,|ui|{
  ui.add_space(12.0);
  ui.vertical_centered(|ui|{ui.label(RichText::new("Seu ponto de encontro").size(24.0).strong());ui.label("Conversas, voz e tela com seus amigos na LAN ou VPN.");});
  ui.add_space(12.0);
  let width=560.0_f32.min(ui.available_width()-32.0);
  ui.horizontal(|ui|{ui.add_space(((ui.available_width()-width)/2.0).max(0.0));ui.vertical(|ui|{
   ui.set_width(width);
   ui.add_space(8.0);
   Frame::new().fill(theme::color(theme::SIDEBAR)).corner_radius(8).inner_margin(Margin::same(16)).show(ui,|ui|{
    ui.heading("Entrar no servidor dos amigos");
    ui.label("Cole o convite recebido. O endereço e o acesso já estão incluídos.");
    ui.label("O administrador pode ler mensagens e anexos, inclusive de conversas privadas. Não há criptografia de ponta a ponta que o exclua.");
    let address=ui.add(egui::TextEdit::singleline(&mut app.join_input).desired_width(f32::INFINITY).hint_text("lanvox://… (convite) ou meu-host:47800"));
    if address.changed() { app.load_host_preferences(&app.join_input.clone()); }
    let mut join=None;
    ui.horizontal(|ui|{
     if ui.add_enabled(!app.connecting&&!app.join_input.trim().is_empty(),egui::Button::new("Entrar")).clicked()||(address.lost_focus()&&ui.input(|i|i.key_pressed(egui::Key::Enter))){join=Some(app.join_input.trim().to_owned());}
     if ui.add_enabled(!app.join_input.trim().is_empty(), egui::Button::new("Salvar favorito")).clicked(){let value=crate::settings::host_address(&app.join_input);if !app.settings.favorites.contains(&value){app.settings.favorites.push(value);app.save_preferences();}}
     if app.connecting{ui.spinner();ui.label(&app.connection_status);if ui.button("Cancelar").clicked(){app.cancel_connection();}}
    });
    ui.collapsing("Detalhes de privacidade", |ui| { ui.label(crate::branding::ADMIN_VISIBILITY_NOTICE); });
    ui.collapsing("Conexão manual e verificação de identidade",|ui|{
     ui.label("Para entrar com um endereço sem convite, informe o token fornecido pelo administrador.");
     ui.add(egui::TextEdit::singleline(&mut app.settings.access_token).password(true).desired_width(f32::INFINITY).hint_text("Token de acesso"));
     ui.label("Confirme a impressão digital do host com o administrador por um canal confiável.");
     ui.add(egui::TextEdit::singleline(&mut app.settings.expected_host).desired_width(f32::INFINITY).hint_text("Impressão digital do host"));
    });
    if let Some(error)=&app.join_error{ui.label(RichText::new(error).color(theme::color(theme::YELLOW)));}
    let favorites=app.settings.favorites.clone();
    for favorite in favorites{ui.horizontal(|ui|{let shown=favorite.split('?').next().unwrap_or(&favorite).trim_start_matches("lanvox://");if ui.link(shown).clicked(){app.join_input=favorite.clone();app.load_host_preferences(&favorite);join=Some(favorite.clone());}
if ui.small_button("Remover").clicked(){app.settings.favorites.retain(|f|f!=&favorite);app.save_preferences();}});}
    let discovered=app.discovery.as_ref().map(|d|d.list()).unwrap_or_default();
    if !discovered.is_empty(){ui.separator();ui.label("Encontrados na LAN");for host in discovered{if ui.button(format!("{} · {}",host.name,host.address)).clicked(){join=Some(host.address);}}}
    else{ui.label(RichText::new("Na VPN, use o endereço acima. A busca automática depende de broadcast local.").small().color(theme::color(theme::TEXT_MUTED)));}
    if let Some(address)=join{app.connect_from_form(&ctx,address);}
   });
   ui.add_space(20.0);
   ui.collapsing("Seu nome e avatar", |ui| {
   ui.label("Seu nome");
   if ui.add(egui::TextEdit::singleline(&mut app.settings.name).char_limit(32).desired_width(f32::INFINITY)).lost_focus(){app.save_preferences();}
   let before = app.settings.color;
   super::components::avatar_color_picker(ui, &mut app.settings.color);
   if before != app.settings.color { app.save_preferences(); }
   });
   ui.add_space(12.0);
   egui::CollapsingHeader::new(if app.hosted.is_some(){"Servidor em execução neste computador"}else{"Hospedar neste computador"}).default_open(app.hosted.is_some()).show(ui, |ui| {
   ui.label("O servidor fica disponível enquanto este aplicativo estiver aberto. Para hospedagem permanente, use o host independente.");
   ui.horizontal(|ui|{ui.label("Nome");ui.add(egui::TextEdit::singleline(&mut app.host_input).char_limit(40));});
   ui.label("Interface que aceitará conexões");
   ui.label(RichText::new("Para amigos na VPN, selecione o IP da VPN. 127.0.0.1 aceita apenas este computador.").small().color(theme::color(theme::TEXT_MUTED)));
   ui.add_enabled_ui(app.hosted.is_none(),|ui|{
   egui::ComboBox::from_id_salt("host-interface").selected_text(&app.settings.host_bind).show_ui(ui,|ui|{
    ui.selectable_value(&mut app.settings.host_bind,"127.0.0.1".into(),"Apenas este computador · 127.0.0.1");
    for (name,address) in crate::notifications::network_addresses(){if address.is_ipv4(){ui.selectable_value(&mut app.settings.host_bind,address.to_string(),format!("{name} · {address}"));}}
   });
   });
   if ui.add_enabled(!app.connecting,egui::Button::new(if app.hosted.is_some(){"Entrar no host local"}else{"Iniciar host privado"})).clicked(){app.host(&ctx);}
   if app.local_host_busy&&ui.button("Entrar no host já em execução").clicked(){app.connect(&ctx,format!("127.0.0.1:{}",crate::protocol::DEFAULT_PORT));}
   if let Some(error)=&app.host_error{ui.label(RichText::new(error).color(theme::color(theme::YELLOW)));}
   });
   ui.add_space(16.0);
   ui.label("Servidor é a conexão; comunidades organizam seus canais dentro dele.");
   if ui.button("Preferências e identidade").clicked(){app.modal=crate::app::Modal::Settings(crate::app::SettingsTab::Account);}
   if ui.button("Ajuda e atalhos · F1").clicked(){ctx.data_mut(|data|data.insert_temp(egui::Id::unique("help-open"),true));}
   ui.add_space(24.0);
  });});
 });
}
