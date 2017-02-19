package model

type ClientMessage struct {
	ModelType
	Msg    string `json:"msg"`
	Sender string `json:"sender"`
}

type ModelType struct {
	MsgType string `json:"type"`
}
