#ifndef XCB_XINPUT_RAW_H
#define XCB_XINPUT_RAW_H

/* Allow building on systems without xcb-xinput (i.e. debian)
 * We only need a few types to handle events.
 */

typedef uint16_t xcb_input_device_id_t;

/** Opcode for xcb_input_raw_key_press. */
#define XCB_INPUT_RAW_KEY_PRESS 13

/**
 * @brief xcb_input_raw_key_press_event_t
 **/
typedef struct xcb_input_raw_key_press_event_t {
    uint8_t               response_type; /**<  */
    uint8_t               extension; /**<  */
    uint16_t              sequence; /**<  */
    uint32_t              length; /**<  */
    uint16_t              event_type; /**<  */
    xcb_input_device_id_t deviceid; /**<  */
    xcb_timestamp_t       time; /**<  */
    uint32_t              detail; /**<  */
    xcb_input_device_id_t sourceid; /**<  */
    uint16_t              valuators_len; /**<  */
    uint32_t              flags; /**<  */
    uint8_t               pad0[4]; /**<  */
    uint32_t              full_sequence; /**<  */
} xcb_input_raw_key_press_event_t;

/** Opcode for xcb_input_raw_key_release. */
#define XCB_INPUT_RAW_KEY_RELEASE 14

typedef xcb_input_raw_key_press_event_t xcb_input_raw_key_release_event_t;

/** Opcode for xcb_input_raw_button_press. */
#define XCB_INPUT_RAW_BUTTON_PRESS 15

/**
 * @brief xcb_input_raw_button_press_event_t
 **/
typedef struct xcb_input_raw_button_press_event_t {
    uint8_t               response_type; /**<  */
    uint8_t               extension; /**<  */
    uint16_t              sequence; /**<  */
    uint32_t              length; /**<  */
    uint16_t              event_type; /**<  */
    xcb_input_device_id_t deviceid; /**<  */
    xcb_timestamp_t       time; /**<  */
    uint32_t              detail; /**<  */
    xcb_input_device_id_t sourceid; /**<  */
    uint16_t              valuators_len; /**<  */
    uint32_t              flags; /**<  */
    uint8_t               pad0[4]; /**<  */
    uint32_t              full_sequence; /**<  */
} xcb_input_raw_button_press_event_t;

/** Opcode for xcb_input_raw_button_release. */
#define XCB_INPUT_RAW_BUTTON_RELEASE 16

typedef xcb_input_raw_button_press_event_t xcb_input_raw_button_release_event_t;

/** Opcode for xcb_input_raw_motion. */
#define XCB_INPUT_RAW_MOTION 17

typedef xcb_input_raw_button_press_event_t xcb_input_raw_motion_event_t;

#endif // XCB_XINPUT_RAW_H
