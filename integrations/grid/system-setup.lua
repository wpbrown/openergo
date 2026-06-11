-- =============================================================================
-- System element setup for the Intech Grid PBF4 used as an Openergo display.
--
-- Two callbacks are installed on `self`:
--   * sysexrx_cb: reply to a custom SysEx "poll" request by sending the
--     current position of each of the four potmeters back as MIDI CC.
--   * midirx_cb:  receive a "heat" value (MIDI CC, 0..127) for each of the
--     12 LEDs and render it as off / green->red gradient / pulsing red alert.
--
-- The PBF4 has 4 potmeters (elements 4..7) and 12 LEDs (indices 0..11). All
-- LEDs are addressed on layer 1; layer 2 is unused.
-- =============================================================================


-- -----------------------------------------------------------------------------
-- SysEx receive: reply to a "poll potmeters" request.
--
-- Incoming SysEx must start with F0 (SysEx start), then 7D (non-commercial
-- manufacturer ID), then 01 (our command byte), and end with F7. When we see
-- it, we read each potmeter's current value and broadcast it as a MIDI CC
-- message on channel 1 (status 0xB0), CC numbers 36..39 (= 32 + element idx).
-- The +32 offset mirrors the CC->LED mapping used by midirx_cb below.
-- -----------------------------------------------------------------------------
self.sysexrx_cb = function(self, header, sysex)
  -- The SysEx payload arrives as an ASCII hex string ("F07D01..F7"); decode
  -- it into a flat array of byte values for easy header checks.
  local bytes = {}
  for hex_byte in sysex:gmatch("%x%x") do
    bytes[#bytes + 1] = tonumber(hex_byte, 16)
  end

  if #bytes >= 4
      and bytes[1] == 0xF0          -- SysEx start
      and bytes[2] == 0x7D          -- "educational / non-commercial" manufacturer ID
      and bytes[3] == 0x01          -- our command: "report potmeter values"
      and bytes[#bytes] == 0xF7     -- SysEx end
  then
    -- Elements 4..7 are the four potmeters on the PBF4. Send each value as a
    -- CC on channel 1 (status 0xB0) so the host sees a normal CC stream.
    for element_index = 4, 7 do
      local pot_value = element[element_index]:potmeter_value()
      midi_send(0, 0xB0, 32 + element_index, pot_value)
    end
  end
end


-- -----------------------------------------------------------------------------
-- MIDI receive: render a "heat" value on one of the 12 LEDs.
--
-- We listen for CC messages from Openergo (header[1] == 13 means the message
-- arrived from the host, i.e. Openergo) on channel 1 (event[1] == 0), status
-- 0xB0 = 176 (CC). CC numbers 32..43 map to LED indices 0..11.
--
-- The CC value is interpreted as a heat level:
--     0          -> LED fully off
--     1 .. 100   -> green -> red gradient, brightness ramps 16 -> 255
--   101 .. 127   -> solid red "alert", pulsing faster as the value grows
-- -----------------------------------------------------------------------------
self.midirx_cb = function(self, header, event)
  -- event[1] = channel, event[2] = command (176 = CC), event[3] = CC number,
  -- event[4] = CC value. header[1] = 13 means message originated from Openergo.
  if header[1] ~= 13 or event[1] ~= 0 or event[2] ~= 176 then
    return
  end

  local led_index = event[3] - 32
  if led_index < 0 or led_index > 11 then
    return
  end

  -- `floor` is aliased to a local so the minifier can shorten the many
  -- math.floor call sites below.
  local cc_value, floor = event[4], math.floor

  -- LED 4 is special: it mirrors its own potmeter in purple by default, and
  -- pulses at full brightness whenever Openergo sends a non-zero CC for it.
  if led_index == 4 then
    -- Purple is the same in both branches; set it once.
    led_color(led_index, 1, 160, 32, 240, 1)
    if cc_value == 0 then
      -- Default: stop any pulse, brightness follows the pot.
      led_animation_phase_rate_type(led_index, 1, 0, 0, 0)
      led_value(led_index, 1, element[4]:potmeter_value())
    else
      -- Alert: max brightness, sine pulse (type 3).
      led_value(led_index, 1, 255)
      led_animation_phase_rate_type(led_index, 1, 0, 1, 3)
    end
    return
  end

  if cc_value <= 0 then
    -- OFF: stop any running animation (phase=0, freq=0, type=0 is the
    -- documented "stop" call), clear color, zero brightness.
    led_animation_phase_rate_type(led_index, 1, 0, 0, 0)
    led_color(led_index, 1, 0, 0, 0)
    led_value(led_index, 1, 0)

  elseif cc_value <= 100 then
    -- HEATMAP: linearly interpolate green->red and 16->255 brightness across
    -- CC values 1..100. `ratio` is 0 at cc=1 and 1 at cc=100.
    local ratio = (cc_value - 1) / 99
    led_animation_phase_rate_type(led_index, 1, 0, 0, 0)
    led_color(
      led_index, 1,
      floor(255 * ratio + 0.5),         -- red:   0 -> 255
      floor(255 - 255 * ratio + 0.5),   -- green: 255 -> 0
      0                                 -- blue:  always 0
    )
    led_value(led_index, 1, floor(16 + 239 * ratio + 0.5))

  else
    -- ALERT: solid red at max brightness with a sine pulse (animation type 3
    -- per the docs). Frequency scales 1..16 across cc 101..127, so higher
    -- values pulse faster. Clamp cc to 127 to keep the formula in range.
    if cc_value > 127 then
      cc_value = 127
    end
    led_color(led_index, 1, 255, 0, 0)
    led_value(led_index, 1, 255)
    led_animation_phase_rate_type(
      led_index, 1,
      0,                                            -- phase
      floor(1 + 15 * (cc_value - 101) / 26 + 0.5),  -- frequency: 1..16
      3                                             -- type 3 = sine pulse
    )
  end
end