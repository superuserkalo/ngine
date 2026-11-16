show_screenrecord_menu() {
  ngine record stop >/dev/null 2>&1 && exit 0

  case $(menu "Screenrecord" "  Start recording\n󰑙  Instant replay on or off\n  Save the last 30 seconds\n  Open ngine") in
  *"Start recording") ngine record start ;;
  *"Instant replay on or off") ngine replay toggle ;;
  *"Save the last 30 seconds") ngine replay save --last 30s ;;
  *"Open ngine") uwsm-app -- ngine ;;
  *) back_to show_capture_menu ;;
  esac
}
