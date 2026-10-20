TITLE="rolling DC loss: dc1, then dc2, then dc3, 20s each with 15s gaps"
HOLD=95
TAIL=20
incident() {
  bin/chaos isolate dc1; sleep 20; bin/chaos heal; sleep 15
  bin/chaos isolate dc2; sleep 20; bin/chaos heal; sleep 15
  bin/chaos isolate dc3; sleep 20
}
