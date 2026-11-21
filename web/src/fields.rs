// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The settings of the device's sections and of the modules, in Spanish:
//! what the protobufs say of each, label and description, put faithfully.
//! In English, and for any field not here, the protobufs' own words.

/// By `section.field`: the label, and the description (empty for none).
const ES: &[(&str, &str, &str)] = &[
    // Device.
    (
        "config.device.button_gpio",
        "GPIO del botón",
        "Pin GPIO del botón de usuario; se puede reasignar en placas con varios botones.",
    ),
    (
        "config.device.buzzer_gpio",
        "GPIO del zumbador",
        "Pin GPIO del zumbador PWM.",
    ),
    (
        "config.device.rebroadcast_mode",
        "Modo de retransmisión",
        "",
    ),
    (
        "config.device.node_info_broadcast_secs",
        "Intervalo de información del nodo",
        "Cada cuánto se envía la información del nodo. Por defecto 3 horas, como mínimo 1 hora.",
    ),
    (
        "config.device.double_tap_as_button_press",
        "Doble toque como botón",
        "Trata un doble toque en los acelerómetros compatibles como una pulsación del botón de usuario.",
    ),
    (
        "config.device.disable_triple_click",
        "Desactivar el triple clic",
        "Desactiva el atajo de pulsar tres veces el botón de usuario.",
    ),
    (
        "config.device.tzdef",
        "Zona horaria",
        "Definición de zona horaria POSIX.",
    ),
    (
        "config.device.led_heartbeat_disabled",
        "Desactivar el latido del LED",
        "Apaga el LED que parpadea en el dispositivo. En la mayoría es uno de hasta 4 LED; los del cargador y el GPS no se pueden controlar.",
    ),
    ("config.device.buzzer_mode", "Modo del zumbador", ""),
    // Position.
    (
        "config.position.position_broadcast_secs",
        "Intervalo de emisión",
        "El tiempo máximo que pasa un nodo sin emitir su posición.",
    ),
    (
        "config.position.position_broadcast_smart_enabled",
        "Posición inteligente",
        "",
    ),
    (
        "config.position.fixed_position",
        "Posición fija",
        "La última latitud, longitud y altitud conocidas se emiten por la malla en cada intervalo de posición, en lugar de una lectura del GPS en vivo.",
    ),
    (
        "config.position.gps_update_interval",
        "Intervalo de actualización",
        "Cada cuánto se intenta obtener una posición del GPS.",
    ),
    (
        "config.position.position_flags",
        "Campos de posición",
        "Campos opcionales que incluir al montar los mensajes de posición.",
    ),
    (
        "config.position.rx_gpio",
        "GPIO de recepción del GPS",
        "Pin GPIO para RX del GPS.",
    ),
    (
        "config.position.tx_gpio",
        "GPIO de transmisión del GPS",
        "Pin GPIO para TX del GPS.",
    ),
    (
        "config.position.broadcast_smart_minimum_distance",
        "Distancia mínima",
        "El cambio mínimo de distancia antes de considerar una emisión inteligente de la posición.",
    ),
    (
        "config.position.broadcast_smart_minimum_interval_secs",
        "Intervalo mínimo",
        "El intervalo más corto entre actualizaciones de posición una vez alcanzada la distancia mínima.",
    ),
    (
        "config.position.gps_en_gpio",
        "GPIO de activación del GPS",
        "Pin GPIO para activar el GPS.",
    ),
    ("config.position.gps_mode", "Modo del GPS", ""),
    // Power.
    (
        "config.power.is_power_saving",
        "Ahorro de energía",
        "Lo duerme todo cuanto puede; con los roles Rastreador y Sensor incluye también la radio LoRa. No lo uses si quieres usar el dispositivo con las apps del móvil o si el dispositivo no tiene botón de usuario.",
    ),
    (
        "config.power.on_battery_shutdown_after_secs",
        "Apagar al perder la alimentación",
        "Cuánto tarda el dispositivo en apagarse tras quitarle la alimentación externa. Cero para desactivarlo; los valores por debajo de 30 segundos se suben a 30.",
    ),
    (
        "config.power.adc_multiplier_override",
        "Corrección del ADC",
        "",
    ),
    (
        "config.power.wait_bluetooth_secs",
        "Espera del Bluetooth",
        "",
    ),
    ("config.power.sds_secs", "Sueño profundo", ""),
    ("config.power.ls_secs", "Sueño ligero", ""),
    ("config.power.min_wake_secs", "Tiempo mínimo despierto", ""),
    (
        "config.power.device_battery_ina_address",
        "Dirección del INA de la batería",
        "",
    ),
    ("config.power.powermon_enables", "Monitor de energía", ""),
    // Network.
    (
        "config.network.wifi_enabled",
        "Wi-Fi activado",
        "Activar el wifi desactiva la conexión Bluetooth con la app.",
    ),
    (
        "config.network.wifi_ssid",
        "SSID",
        "Nombre de la red wifi a la que conectarse.",
    ),
    (
        "config.network.wifi_psk",
        "Contraseña",
        "Contraseña de la red wifi.",
    ),
    (
        "config.network.ntp_server",
        "Servidor NTP",
        "Dirección del servidor NTP. Por defecto meshtastic.pool.ntp.org.",
    ),
    (
        "config.network.eth_enabled",
        "Ethernet activado",
        "En las placas ESP32, activar Ethernet desactiva la conexión Bluetooth con la app.",
    ),
    ("config.network.address_mode", "Modo de dirección", ""),
    ("config.network.ipv4_config.ip", "IP", ""),
    ("config.network.ipv4_config.gateway", "Puerta de enlace", ""),
    ("config.network.ipv4_config.subnet", "Subred", ""),
    ("config.network.ipv4_config.dns", "DNS", ""),
    ("config.network.rsyslog_server", "Servidor rsyslog", ""),
    (
        "config.network.enabled_protocols",
        "Protocolos activados",
        "Emitir paquetes por UDP en la red local.",
    ),
    ("config.network.ipv6_enabled", "IPv6 activado", ""),
    // Display.
    (
        "config.display.screen_on_secs",
        "Pantalla encendida durante",
        "Cuánto sigue encendida la pantalla tras pulsar el botón de usuario o recibir mensajes.",
    ),
    (
        "config.display.auto_screen_carousel_secs",
        "Intervalo del carrusel",
        "Pasa sola a la siguiente página de la pantalla, como un carrusel, con este intervalo.",
    ),
    (
        "config.display.flip_screen",
        "Girar la pantalla",
        "Gira la pantalla en vertical.",
    ),
    (
        "config.display.units",
        "Unidades",
        "Unidades que se muestran en la pantalla del dispositivo.",
    ),
    (
        "config.display.oled",
        "Tipo de OLED",
        "Sustituye la detección automática de la pantalla OLED.",
    ),
    (
        "config.display.displaymode",
        "Modo de pantalla",
        "Sustituye la disposición de pantalla por defecto.",
    ),
    (
        "config.display.heading_bold",
        "Encabezado en negrita",
        "Pone en negrita el texto del encabezado de la pantalla.",
    ),
    (
        "config.display.wake_on_tap_or_motion",
        "Despertar la pantalla con un toque o movimiento",
        "Requiere que el dispositivo tenga acelerómetro.",
    ),
    (
        "config.display.compass_orientation",
        "Orientación de la brújula",
        "Indica cómo girar o invertir la salida de la brújula para que se muestre bien.",
    ),
    (
        "config.display.use_12h_clock",
        "Reloj de 12 horas",
        "Pone el reloj de la pantalla en formato de 12 horas.",
    ),
    (
        "config.display.use_long_node_name",
        "Nombre largo del nodo",
        "",
    ),
    (
        "config.display.enable_message_bubbles",
        "Burbujas de mensajes",
        "",
    ),
    // Bluetooth.
    (
        "config.bluetooth.enabled",
        "Bluetooth activado",
        "Activa el Bluetooth en el dispositivo.",
    ),
    (
        "config.bluetooth.mode",
        "Modo de emparejamiento",
        "Forma de emparejar por Bluetooth.",
    ),
    (
        "config.bluetooth.fixed_pin",
        "PIN fijo",
        "PIN fijo para emparejar por Bluetooth. Se usa cuando el modo de emparejamiento es PIN fijo.",
    ),
    // Ambient lighting.
    (
        "module.ambient_lighting.led_state",
        "Estado del LED",
        "El estado del LED (encendido o apagado).",
    ),
    (
        "module.ambient_lighting.current",
        "Corriente",
        "Corriente con la que se alimenta la salida del LED.",
    ),
    (
        "module.ambient_lighting.red",
        "Rojo",
        "El nivel de rojo del LED de luz ambiental.",
    ),
    (
        "module.ambient_lighting.green",
        "Verde",
        "El nivel de verde del LED de luz ambiental.",
    ),
    (
        "module.ambient_lighting.blue",
        "Azul",
        "El nivel de azul del LED de luz ambiental.",
    ),
    // Audio.
    (
        "module.audio.codec2_enabled",
        "Codec2 activado",
        "Activa la codificación y decodificación de audio Codec2 para comunicarse por voz a través de la malla.",
    ),
    (
        "module.audio.ptt_pin",
        "Pin PTT",
        "Número de pin GPIO del pulsar para hablar. 0 usa el GPIO 39.",
    ),
    (
        "module.audio.bitrate",
        "Tasa de bits",
        "La tasa de bits de Codec2. La frecuencia de muestreo es siempre 8 kHz. Las tasas más bajas usan menos ancho de banda, pero reducen la calidad del audio.",
    ),
    ("module.audio.i2s_ws", "Pin de selección de palabra I2S", ""),
    ("module.audio.i2s_sd", "Pin de entrada de datos I2S", ""),
    ("module.audio.i2s_din", "Pin de salida de datos I2S", ""),
    ("module.audio.i2s_sck", "Pin de reloj I2S", ""),
    // Canned messages.
    (
        "module.canned_message.rotary1_enabled",
        "Codificador rotatorio",
        "Activa el codificador rotatorio.",
    ),
    (
        "module.canned_message.inputbroker_pin_a",
        "Pin A",
        "Pin GPIO del puerto A del codificador rotatorio.",
    ),
    (
        "module.canned_message.inputbroker_pin_b",
        "Pin B",
        "Pin GPIO del puerto B del codificador rotatorio.",
    ),
    (
        "module.canned_message.inputbroker_pin_press",
        "Pin de pulsación",
        "Pin GPIO del botón del codificador.",
    ),
    (
        "module.canned_message.inputbroker_event_cw",
        "Evento al girar a la derecha",
        "Evento de entrada al girar en el sentido de las agujas del reloj.",
    ),
    (
        "module.canned_message.inputbroker_event_ccw",
        "Evento al girar a la izquierda",
        "Evento de entrada al girar en sentido contrario a las agujas del reloj.",
    ),
    (
        "module.canned_message.inputbroker_event_press",
        "Evento al pulsar el codificador",
        "Evento de entrada al pulsar el codificador.",
    ),
    (
        "module.canned_message.updown1_enabled",
        "Entrada arriba, abajo y selección",
        "Activa la entrada arriba, abajo y selección.",
    ),
    (
        "module.canned_message.send_bell",
        "Enviar campana",
        "Envía el carácter de campana con los mensajes.",
    ),
    // Detection sensor.
    (
        "module.detection_sensor.enabled",
        "Sensor de detección activado",
        "Activa el módulo del sensor de detección. Tiene que estar activado tanto en el nodo con el sensor como en los nodos que quieran recibir sus mensajes o ver su registro y su gráfica.",
    ),
    (
        "module.detection_sensor.minimum_broadcast_secs",
        "Tiempo mínimo entre avisos de detección",
        "Tiempo mínimo entre los avisos de detección.",
    ),
    (
        "module.detection_sensor.state_broadcast_secs",
        "Intervalo de emisión del estado",
        "Cada cuánto se envía el estado del sensor a la malla, se haya detectado algo o no. 0 solo lo envía al detectar.",
    ),
    (
        "module.detection_sensor.send_bell",
        "Enviar campana",
        "Envía la campana ASCII con el mensaje de aviso. Útil para disparar un aviso externo con la campana.",
    ),
    (
        "module.detection_sensor.name",
        "Nombre",
        "Nombre del sensor en los mensajes de la malla.",
    ),
    (
        "module.detection_sensor.monitor_pin",
        "Pin GPIO que vigilar",
        "Pin GPIO cuyos cambios de estado se vigilan. 0 desactiva el módulo.",
    ),
    (
        "module.detection_sensor.detection_trigger_type",
        "Tipo de disparo",
        "Tipo de evento que lo dispara.",
    ),
    (
        "module.detection_sensor.use_pullup",
        "Usar la resistencia pull-up interna",
        "Activa la resistencia pull-up interna del pin vigilado.",
    ),
    // External notification.
    (
        "module.external_notification.enabled",
        "Avisos externos activados",
        "Activa los avisos externos.",
    ),
    (
        "module.external_notification.output_ms",
        "Duración de la salida GPIO",
        "En modo GPIO, cuánto tiempo se mantiene activa la salida.",
    ),
    (
        "module.external_notification.output",
        "Pin GPIO de salida",
        "Pin GPIO que se activa con el aviso. Por defecto, el pin EXT_NOTIFY_OUT de la placa.",
    ),
    (
        "module.external_notification.active",
        "Activo en alto",
        "Activado, el pin de salida se pone en alto al activarse; desactivado, en bajo.",
    ),
    (
        "module.external_notification.alert_message",
        "Avisar al recibir un mensaje",
        "Avisa cuando llega un mensaje.",
    ),
    (
        "module.external_notification.alert_bell",
        "Avisar al recibir una campana",
        "Avisa con el carácter de campana.",
    ),
    (
        "module.external_notification.use_pwm",
        "Usar zumbador PWM",
        "Usa una salida PWM (como el zumbador RAK) para las melodías en lugar de una salida de encendido y apagado. Ignora los ajustes de salida, duración y activo, y usa el GPIO del zumbador de la configuración del dispositivo.",
    ),
    (
        "module.external_notification.output_vibra",
        "Pin GPIO del vibrador",
        "Pin de salida del motor de vibración.",
    ),
    (
        "module.external_notification.output_buzzer",
        "Pin GPIO del zumbador",
        "Pin de salida del zumbador.",
    ),
    (
        "module.external_notification.alert_message_vibra",
        "Vibrar al recibir un mensaje",
        "Activa el motor de vibración GPIO cuando llega un mensaje.",
    ),
    (
        "module.external_notification.alert_message_buzzer",
        "Zumbador al recibir un mensaje",
        "Suena cuando llega un mensaje.",
    ),
    (
        "module.external_notification.alert_bell_vibra",
        "Vibrar al recibir una campana",
        "Vibra con el carácter de campana.",
    ),
    (
        "module.external_notification.alert_bell_buzzer",
        "Zumbador al recibir una campana",
        "Suena con el carácter de campana.",
    ),
    (
        "module.external_notification.nag_timeout",
        "Duración del aviso",
        "Cuánto dura el aviso.",
    ),
    (
        "module.external_notification.use_i2s_as_buzzer",
        "Usar I2S como zumbador",
        "Permite que los dispositivos con salida de audio I2S propia usen el altavoz para las melodías RTTTL como si fuera un zumbador. El T-Watch S3 y el T-Deck, por ejemplo, pueden hacerlo.",
    ),
    // Mesh beacon.
    ("module.mesh_beacon.flags", "Opciones", ""),
    (
        "module.mesh_beacon.broadcast_offer_frequency_slot",
        "Ranura de frecuencia ofrecida",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_message",
        "Mensaje",
        "Mensaje de las emisiones de la baliza, hasta 60 bytes.",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.name",
        "Nombre del canal ofrecido",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.id",
        "Id del canal ofrecido",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.uplink_enabled",
        "El canal ofrecido sube a MQTT",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.downlink_enabled",
        "El canal ofrecido baja de MQTT",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.module_settings.position_precision",
        "Precisión de posición del canal ofrecido",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.module_settings.is_muted",
        "Canal ofrecido silenciado",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_channel.use_aead",
        "Cifrado autenticado del canal ofrecido",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_region",
        "Región ofrecida",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_offer_preset",
        "Preset ofrecido",
        "",
    ),
    (
        "module.mesh_beacon.broadcast_interval_secs",
        "Intervalo",
        "Cada cuánto se emite la baliza. Como mínimo 1 hora.",
    ),
    // MQTT.
    (
        "module.mqtt.enabled",
        "MQTT activado",
        "Activa la pasarela MQTT.",
    ),
    (
        "module.mqtt.address",
        "Dirección",
        "Dirección del servidor MQTT.",
    ),
    ("module.mqtt.username", "Usuario", "Usuario de MQTT."),
    ("module.mqtt.password", "Contraseña", "Contraseña de MQTT."),
    (
        "module.mqtt.encryption_enabled",
        "Cifrado activado",
        "Envía los paquetes cifrados a MQTT. Desactivado, el servidor y cualquiera que pueda publicar en él son de tanta confianza como los miembros del canal.",
    ),
    (
        "module.mqtt.tls_enabled",
        "TLS activado",
        "Se conecta al servidor MQTT por TLS.",
    ),
    ("module.mqtt.root", "Tema raíz", "Tema raíz de MQTT."),
    (
        "module.mqtt.proxy_to_client_enabled",
        "MQTT a través del cliente",
        "Usa la conexión de red de tu móvil para conectarse a MQTT.",
    ),
    (
        "module.mqtt.map_reporting_enabled",
        "Informes para el mapa",
        "Tu nodo enviará cada cierto tiempo un informe para el mapa, sin cifrar, al servidor MQTT configurado. Incluye id, nombre corto y largo, posición aproximada, modelo de hardware, rol, versión del firmware, región LoRa, preset del módem y nombre del canal principal.",
    ),
    (
        "module.mqtt.map_report_settings.publish_interval_secs",
        "Intervalo de publicación en el mapa",
        "Cada cuánto se publica un informe para el mapa. Como mínimo 1 hora.",
    ),
    (
        "module.mqtt.map_report_settings.position_precision",
        "Precisión de la posición en el mapa",
        "",
    ),
    (
        "module.mqtt.map_report_settings.should_report_location",
        "Compartir en el mapa",
        "Consentimiento para publicar sin cifrar los datos de este nodo y su posición aproximada en el mapa de MQTT. Sin él no se envía ningún informe.",
    ),
    // Neighbour info.
    (
        "module.neighbor_info.enabled",
        "Información de vecinos activada",
        "Activa la emisión de información de vecinos. Envía cada cierto tiempo información sobre los vecinos que se oyen directamente, para ayudar a ver la topología de la malla.",
    ),
    (
        "module.neighbor_info.update_interval",
        "Intervalo de actualización",
        "Cada cuánto se emite la información de vecinos. Como mínimo 4 horas.",
    ),
    (
        "module.neighbor_info.transmit_over_lora",
        "Transmitir por LoRa",
        "Si se transmite la información de vecinos por LoRa además de por MQTT y a la app conectada. No disponible en canales con la clave y el nombre por defecto.",
    ),
    // People counter.
    (
        "module.paxcounter.enabled",
        "Contador de personas activado",
        "Activado, el contador de personas cuenta los dispositivos wifi y Bluetooth cercanos. Para que funcione, el wifi y el Bluetooth tienen que estar desactivados.",
    ),
    (
        "module.paxcounter.paxcounter_update_interval",
        "Intervalo de actualización",
        "Cada cuánto se envían los recuentos a la malla.",
    ),
    (
        "module.paxcounter.wifi_threshold",
        "Umbral del wifi",
        "Umbral de RSSI para contar dispositivos wifi.",
    ),
    (
        "module.paxcounter.ble_threshold",
        "Umbral de BLE",
        "Umbral de RSSI para contar dispositivos BLE.",
    ),
    // Range test.
    (
        "module.range_test.enabled",
        "Prueba de alcance activada",
        "Activa el módulo de prueba de alcance.",
    ),
    (
        "module.range_test.sender",
        "Intervalo de envío",
        "Este dispositivo enviará mensajes de prueba de alcance con el intervalo elegido. 0 desactiva el envío.",
    ),
    (
        "module.range_test.save",
        "Guardar",
        "Guarda un CSV con los detalles de los mensajes de prueba de alcance; de momento, solo en dispositivos ESP32 con servidor web.",
    ),
    (
        "module.range_test.clear_on_reboot",
        "Borrar al reiniciar",
        "",
    ),
    // Remote hardware.
    (
        "module.remote_hardware.enabled",
        "Hardware remoto activado",
        "",
    ),
    (
        "module.remote_hardware.allow_undefined_pin_access",
        "Permitir acceso a pines no definidos",
        "",
    ),
    // Serial.
    (
        "module.serial.enabled",
        "Puerto serie activado",
        "Activa el módulo del puerto serie.",
    ),
    (
        "module.serial.echo",
        "Eco",
        "Si se activa, los paquetes que envíes se devuelven por el puerto serie.",
    ),
    (
        "module.serial.rxd",
        "Pin GPIO de recepción (rxd)",
        "Número de pin RX.",
    ),
    (
        "module.serial.txd",
        "Pin GPIO de transmisión (txd)",
        "Número de pin TX.",
    ),
    (
        "module.serial.baud",
        "Baudios",
        "Velocidad del puerto serie en baudios.",
    ),
    (
        "module.serial.timeout",
        "Tiempo de espera",
        "Cuánto esperar antes de tratar los datos serie recibidos como un paquete completo. 0 usa 250 ms.",
    ),
    (
        "module.serial.mode",
        "Modo",
        "Modo de funcionamiento del módulo del puerto serie.",
    ),
    (
        "module.serial.override_console_serial_port",
        "Usar el puerto serie de la consola",
        "",
    ),
    // Store and forward.
    (
        "module.store_forward.enabled",
        "Guardar y reenviar activado",
        "Activa el módulo de guardar y reenviar.",
    ),
    (
        "module.store_forward.heartbeat",
        "Enviar latido",
        "Envía un latido para anunciar que el servidor está ahí.",
    ),
    (
        "module.store_forward.records",
        "Número de registros",
        "Número máximo de mensajes que guardar. 0 usa tres cuartas partes de la PSRAM libre.",
    ),
    (
        "module.store_forward.history_return_max",
        "Máximo de historial devuelto",
        "Número máximo de mensajes guardados que se devuelven por petición. 0 usa 25.",
    ),
    (
        "module.store_forward.history_return_window",
        "Ventana del historial devuelto",
        "Hasta cuánto tiempo atrás, en minutos, se devuelven los mensajes guardados. 0 usa 240.",
    ),
    (
        "module.store_forward.is_server",
        "Servidor",
        "Hace de este dispositivo un servidor de guardar y reenviar. Requiere un ESP32 con PSRAM.",
    ),
    // TAK.
    ("module.tak.team", "Equipo", "Color del equipo TAK."),
    ("module.tak.role", "Rol", "Rol del miembro TAK."),
    // Telemetry.
    (
        "module.telemetry.device_update_interval",
        "Intervalo de métricas del dispositivo",
        "Cada cuánto se envían las métricas del dispositivo por la malla.",
    ),
    (
        "module.telemetry.environment_update_interval",
        "Intervalo de métricas del entorno",
        "Cada cuánto se envían las métricas del entorno por la malla.",
    ),
    (
        "module.telemetry.environment_measurement_enabled",
        "Métricas del entorno activadas",
        "Recoge medidas del entorno.",
    ),
    (
        "module.telemetry.environment_screen_enabled",
        "Mostrar en la pantalla del dispositivo",
        "Muestra las medidas del entorno en el dispositivo.",
    ),
    (
        "module.telemetry.environment_display_fahrenheit",
        "Mostrar en Fahrenheit",
        "Muestra el entorno en grados Fahrenheit.",
    ),
    (
        "module.telemetry.air_quality_enabled",
        "Métricas de calidad del aire activadas",
        "Recoge métricas de calidad del aire.",
    ),
    (
        "module.telemetry.air_quality_interval",
        "Intervalo de calidad del aire",
        "Cada cuánto se envían las métricas de calidad del aire por la malla.",
    ),
    (
        "module.telemetry.power_measurement_enabled",
        "Medidas de energía activadas",
        "Recoge métricas de energía.",
    ),
    (
        "module.telemetry.power_update_interval",
        "Intervalo de métricas de energía",
        "Cada cuánto se envían las métricas de energía por la malla.",
    ),
    (
        "module.telemetry.power_screen_enabled",
        "Pantalla de energía",
        "Muestra las métricas de energía en el dispositivo.",
    ),
    (
        "module.telemetry.health_measurement_enabled",
        "Medidas de salud activadas",
        "",
    ),
    (
        "module.telemetry.health_update_interval",
        "Intervalo de métricas de salud",
        "",
    ),
    (
        "module.telemetry.health_screen_enabled",
        "Pantalla de salud",
        "",
    ),
    (
        "module.telemetry.device_telemetry_enabled",
        "Emitir las métricas del dispositivo",
        "Emite las métricas del dispositivo a la malla. Desactivado, solo se envían a los clientes conectados.",
    ),
    (
        "module.telemetry.air_quality_screen_enabled",
        "Pantalla de calidad del aire",
        "",
    ),
    // Traffic management.
    (
        "module.traffic_management.position_min_interval_secs",
        "Intervalo mínimo de posición",
        "Las posiciones de un mismo nodo que lleguen antes de este tiempo se descartan.",
    ),
    (
        "module.traffic_management.nodeinfo_direct_response_max_hops",
        "Saltos máximos para responder NodeInfo",
        "Responde desde la caché a las peticiones de NodeInfo de quienes estén a este número de saltos o menos. Como mucho 3 en los routers; los demás roles solo responden a los vecinos directos.",
    ),
    (
        "module.traffic_management.rate_limit_window_secs",
        "Ventana del límite de envío",
        "El tiempo durante el que se cuentan los paquetes.",
    ),
    (
        "module.traffic_management.rate_limit_max_packets",
        "Máximo de paquetes por ventana",
        "El máximo de paquetes que puede enviar un nodo en cada ventana.",
    ),
    (
        "module.traffic_management.unknown_packet_threshold",
        "Umbral de paquetes indescifrables",
        "Paquetes indescifrables que puede enviar un nodo en 5 minutos antes de que se descarten.",
    ),
];

/// A field's label and description in Spanish, if here.
pub fn spanish(id: &str) -> Option<(&'static str, &'static str)> {
    ES.iter()
        .find(|(known, _, _)| *known == id)
        .map(|(_, label, description)| (*label, *description))
}
