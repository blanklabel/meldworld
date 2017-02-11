package model

type ClientMessage struct {
	MsgType string `json:"type"`
	Msg     string `json:"msg"`
	Sender  string `json:"sender"`
}

type ClientAction struct {
}

type ModelType struct {
	MsgType string `json:"type"`
}
